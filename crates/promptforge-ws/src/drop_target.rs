//! A delegating OLE drop target for the WebView2 child windows.
//!
//! wry's `with_drag_drop_handler` revokes WebView2's own `IDropTarget` and
//! installs a replacement that never talks back to Chromium. That kills
//! every HTML5 drag-and-drop interaction inside the page - Chromium routes
//! even in-page drags through the OS drag loop, so the revoked target means
//! `dragover`/`drop` never fire and Dockview panels stop being draggable.
//!
//! The shell instead wraps Chromium's target: [`install`] finds each child
//! window that already registered a drop target (OLE stores it in the
//! `OleDropTargetInterface` window property), revokes it, and re-registers
//! a wrapper that forwards every call to the original. The page keeps full
//! drag-and-drop fidelity; the wrapper merely observes `CF_HDROP` paths on
//! `Drop` and reports them, which feeds the same `promptforge:file-drop`
//! grant flow the wry handler used to. The page suppresses the default
//! open-the-file navigation itself with `preventDefault` on file drags.
//!
//! Chromium registers its drop target asynchronously after the webview is
//! created, so [`install`] returns `None` until a target exists and the
//! caller retries. If no target ever appears, nothing is intercepted:
//! drag-and-drop inside the page still works and only the native path
//! grants degrade.

use std::ffi::{OsString, c_void};
use std::os::windows::ffi::OsStringExt as _;
use std::path::PathBuf;
use std::rc::Rc;

use windows::Win32::Foundation::{HWND, LPARAM, POINTL};
use windows::Win32::System::Com::{DVASPECT_CONTENT, FORMATETC, IDataObject, TYMED_HGLOBAL};
use windows::Win32::System::Ole::{
    CF_HDROP, DROPEFFECT, IDropTarget, IDropTarget_Impl, RegisterDragDrop, RevokeDragDrop,
};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::Shell::{DragFinish, DragQueryFileW, HDROP};
use windows::Win32::UI::WindowsAndMessaging::{EnumChildWindows, GetPropW};
use windows::core::{BOOL, Interface as _, Ref, implement, w};

/// How many installation attempts an [`Installer`] makes before giving up.
/// At the caller's 200ms retry cadence this is about 30 seconds - far past
/// any realistic Chromium startup.
const MAX_ATTEMPTS: u32 = 150;

/// Retries the delegation install until Chromium's drop target appears.
pub(crate) struct Installer {
    hwnd: isize,
    on_drop: Rc<dyn Fn(Vec<PathBuf>)>,
    delegation: Option<DropDelegation>,
    attempts: u32,
}

impl Installer {
    pub(crate) fn new(hwnd: isize, on_drop: Rc<dyn Fn(Vec<PathBuf>)>) -> Self {
        Self {
            hwnd,
            on_drop,
            delegation: None,
            attempts: 0,
        }
    }

    /// One installation attempt. Returns `true` when the caller should
    /// schedule another attempt; `false` once installed or given up (drag
    /// keeps working either way, only Explorer path drops degrade).
    pub(crate) fn attempt(&mut self) -> bool {
        if self.delegation.is_some() || self.attempts >= MAX_ATTEMPTS {
            return false;
        }
        self.delegation = install(self.hwnd, &self.on_drop);
        if self.delegation.is_some() {
            return false;
        }
        self.attempts += 1;
        if self.attempts >= MAX_ATTEMPTS {
            eprintln!(
                "no WebView2 drop target appeared; Explorer drops will not grant workspace roots"
            );
            return false;
        }
        true
    }
}

/// The installed delegation: keeps the wrapper targets alive alongside the
/// references OLE itself holds.
struct DropDelegation {
    _targets: Vec<IDropTarget>,
}

/// Wraps the drop target Chromium registered on each child window of
/// `parent`, forwarding all drag traffic to it while reporting dropped
/// `CF_HDROP` paths through `on_drop`.
///
/// Returns `None` when no child window has a registered drop target yet;
/// the caller retries until Chromium finishes its asynchronous setup.
fn install(parent: isize, on_drop: &Rc<dyn Fn(Vec<PathBuf>)>) -> Option<DropDelegation> {
    // EnumChildWindows takes a plain function pointer, so the closure rides
    // through LPARAM as a pointer to its own trait object.
    unsafe extern "system" fn enumerate(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let closure =
            unsafe { &mut *(lparam.0 as *mut c_void).cast::<&mut dyn FnMut(HWND) -> bool>() };
        closure(hwnd).into()
    }

    let mut targets: Vec<IDropTarget> = Vec::new();
    let mut callback = |hwnd: HWND| -> bool {
        wrap_target(hwnd, on_drop, &mut targets);
        true
    };
    let mut trait_obj: &mut dyn FnMut(HWND) -> bool = &mut callback;
    let closure_pointer: *mut c_void = std::ptr::from_mut(&mut trait_obj).cast();
    let _ = unsafe {
        EnumChildWindows(
            Some(HWND(parent as *mut c_void)),
            Some(enumerate),
            LPARAM(closure_pointer as isize),
        )
    };

    if targets.is_empty() {
        None
    } else {
        Some(DropDelegation { _targets: targets })
    }
}

/// Replaces the drop target registered on one window, if any, with a
/// delegating wrapper around it.
fn wrap_target(hwnd: HWND, on_drop: &Rc<dyn Fn(Vec<PathBuf>)>, targets: &mut Vec<IDropTarget>) {
    // OLE keeps the registered target in this window property; a window
    // without one has nothing to delegate to.
    let existing = unsafe { GetPropW(hwnd, w!("OleDropTargetInterface")) };
    let raw = existing.0;
    if raw.is_null() {
        return;
    }
    let Some(original) = (unsafe { IDropTarget::from_raw_borrowed(&raw) }) else {
        return;
    };
    // Clone before the revoke: RevokeDragDrop releases OLE's own reference.
    let original = original.clone();
    if unsafe { RevokeDragDrop(hwnd) }.is_err() {
        return;
    }
    let wrapper: IDropTarget = DelegatingDropTarget {
        inner: original.clone(),
        on_drop: Rc::clone(on_drop),
    }
    .into();
    if unsafe { RegisterDragDrop(hwnd, &wrapper) }.is_ok() {
        targets.push(wrapper);
    } else {
        // The window keeps working without interception: put the original
        // target back so Chromium's drag-and-drop is never degraded.
        let _ = unsafe { RegisterDragDrop(hwnd, &original) };
    }
}

/// Forwards every `IDropTarget` call to the target Chromium registered,
/// additionally reporting the file paths of a `CF_HDROP` drop.
#[implement(IDropTarget)]
struct DelegatingDropTarget {
    inner: IDropTarget,
    on_drop: Rc<dyn Fn(Vec<PathBuf>)>,
}

#[allow(non_snake_case)]
impl IDropTarget_Impl for DelegatingDropTarget_Impl {
    fn DragEnter(
        &self,
        pDataObj: Ref<'_, IDataObject>,
        grfKeyState: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        pdwEffect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        unsafe {
            self.inner
                .DragEnter(pDataObj.ok()?, grfKeyState, *pt, pdwEffect)
        }
    }

    fn DragOver(
        &self,
        grfKeyState: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        pdwEffect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        unsafe { self.inner.DragOver(grfKeyState, *pt, pdwEffect) }
    }

    fn DragLeave(&self) -> windows::core::Result<()> {
        unsafe { self.inner.DragLeave() }
    }

    fn Drop(
        &self,
        pDataObj: Ref<'_, IDataObject>,
        grfKeyState: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        pdwEffect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        let data = pDataObj.ok()?;
        let paths = unsafe { dropped_paths(data) };
        if !paths.is_empty() {
            (self.on_drop)(paths);
        }
        unsafe { self.inner.Drop(data, grfKeyState, *pt, pdwEffect) }
    }
}

/// Extracts the file paths of a `CF_HDROP` drop, or an empty vector when
/// the dragged data is not files. `GetData` hands back a copy of the drop
/// medium, so reading it here never consumes the data Chromium receives
/// when the call is forwarded.
unsafe fn dropped_paths(data: &IDataObject) -> Vec<PathBuf> {
    let drop_format = FORMATETC {
        cfFormat: CF_HDROP.0,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    };
    // A non-file drag (text, in-page HTML5 payloads) has no CF_HDROP and
    // fails here, which is the "not files" answer, not an error.
    let Ok(medium) = (unsafe { data.GetData(&raw const drop_format) }) else {
        return Vec::new();
    };
    let hdrop = HDROP(unsafe { medium.u.hGlobal }.0.cast());
    // 0xFFFFFFFF asks for the item count instead of one item's text.
    let count = unsafe { DragQueryFileW(hdrop, 0xFFFF_FFFF, None) };
    let mut paths = Vec::with_capacity(count as usize);
    for index in 0..count {
        let length = unsafe { DragQueryFileW(hdrop, index, None) } as usize;
        let mut buffer = vec![0u16; length + 1];
        unsafe { DragQueryFileW(hdrop, index, Some(&mut buffer)) };
        paths.push(OsString::from_wide(&buffer[..length]).into());
    }
    unsafe { DragFinish(hdrop) };
    paths
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::path::PathBuf;
    use std::rc::Rc;

    use windows::Win32::Foundation::{DV_E_FORMATETC, E_NOTIMPL, POINTL};
    use windows::Win32::System::Com::{
        FORMATETC, IAdviseSink, IDataObject, IDataObject_Impl, IEnumFORMATETC, IEnumSTATDATA,
        STGMEDIUM,
    };
    use windows::Win32::System::Ole::{
        DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_NONE, IDropTarget, IDropTarget_Impl,
    };
    use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
    use windows::core::{BOOL, HRESULT, Ref, implement};

    use super::DelegatingDropTarget;

    /// A stand-in for Chromium's drop target that records which calls
    /// reached it.
    #[implement(IDropTarget)]
    struct RecordingTarget {
        calls: Rc<RefCell<Vec<&'static str>>>,
    }

    #[allow(non_snake_case)]
    impl IDropTarget_Impl for RecordingTarget_Impl {
        fn DragEnter(
            &self,
            _pDataObj: Ref<'_, IDataObject>,
            _grfKeyState: MODIFIERKEYS_FLAGS,
            _pt: &POINTL,
            pdwEffect: *mut DROPEFFECT,
        ) -> windows::core::Result<()> {
            self.calls.borrow_mut().push("enter");
            unsafe { *pdwEffect = DROPEFFECT_COPY };
            Ok(())
        }

        fn DragOver(
            &self,
            _grfKeyState: MODIFIERKEYS_FLAGS,
            _pt: &POINTL,
            pdwEffect: *mut DROPEFFECT,
        ) -> windows::core::Result<()> {
            self.calls.borrow_mut().push("over");
            unsafe { *pdwEffect = DROPEFFECT_COPY };
            Ok(())
        }

        fn DragLeave(&self) -> windows::core::Result<()> {
            self.calls.borrow_mut().push("leave");
            Ok(())
        }

        fn Drop(
            &self,
            _pDataObj: Ref<'_, IDataObject>,
            _grfKeyState: MODIFIERKEYS_FLAGS,
            _pt: &POINTL,
            pdwEffect: *mut DROPEFFECT,
        ) -> windows::core::Result<()> {
            self.calls.borrow_mut().push("drop");
            unsafe { *pdwEffect = DROPEFFECT_COPY };
            Ok(())
        }
    }

    /// A data object carrying nothing: every format query fails, which is
    /// what an in-page HTML5 drag looks like to the CF_HDROP probe.
    #[implement(IDataObject)]
    struct EmptyDataObject;

    #[allow(non_snake_case)]
    impl IDataObject_Impl for EmptyDataObject_Impl {
        fn GetData(&self, _pformatetcin: *const FORMATETC) -> windows::core::Result<STGMEDIUM> {
            Err(DV_E_FORMATETC.into())
        }
        fn GetDataHere(
            &self,
            _pformatetc: *const FORMATETC,
            _pmedium: *mut STGMEDIUM,
        ) -> windows::core::Result<()> {
            Err(E_NOTIMPL.into())
        }
        fn QueryGetData(&self, _pformatetc: *const FORMATETC) -> HRESULT {
            DV_E_FORMATETC
        }
        fn GetCanonicalFormatEtc(
            &self,
            _pformatectin: *const FORMATETC,
            _pformatetcout: *mut FORMATETC,
        ) -> HRESULT {
            E_NOTIMPL
        }
        fn SetData(
            &self,
            _pformatetc: *const FORMATETC,
            _pmedium: *const STGMEDIUM,
            _frelease: BOOL,
        ) -> windows::core::Result<()> {
            Err(E_NOTIMPL.into())
        }
        fn EnumFormatEtc(&self, _dwdirection: u32) -> windows::core::Result<IEnumFORMATETC> {
            Err(E_NOTIMPL.into())
        }
        fn DAdvise(
            &self,
            _pformatetc: *const FORMATETC,
            _advf: u32,
            _padvsink: Ref<'_, IAdviseSink>,
        ) -> windows::core::Result<u32> {
            Err(E_NOTIMPL.into())
        }
        fn DUnadvise(&self, _dwconnection: u32) -> windows::core::Result<()> {
            Err(E_NOTIMPL.into())
        }
        fn EnumDAdvise(&self) -> windows::core::Result<IEnumSTATDATA> {
            Err(E_NOTIMPL.into())
        }
    }

    fn wrapper_around(
        calls: &Rc<RefCell<Vec<&'static str>>>,
        dropped: &Rc<RefCell<Vec<Vec<PathBuf>>>>,
    ) -> IDropTarget {
        let inner: IDropTarget = RecordingTarget {
            calls: Rc::clone(calls),
        }
        .into();
        let sink = Rc::clone(dropped);
        DelegatingDropTarget {
            inner,
            on_drop: Rc::new(move |paths| sink.borrow_mut().push(paths)),
        }
        .into()
    }

    #[test]
    fn every_drag_call_is_forwarded_to_the_original_target() {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let dropped = Rc::new(RefCell::new(Vec::new()));
        let wrapper = wrapper_around(&calls, &dropped);
        let data: IDataObject = EmptyDataObject.into();
        let point = POINTL { x: 4, y: 8 };
        let mut effect = DROPEFFECT_NONE;

        unsafe {
            wrapper
                .DragEnter(&data, MODIFIERKEYS_FLAGS(0), point, &raw mut effect)
                .expect("DragEnter forwards");
            wrapper
                .DragOver(MODIFIERKEYS_FLAGS(0), point, &raw mut effect)
                .expect("DragOver forwards");
            wrapper.DragLeave().expect("DragLeave forwards");
            wrapper
                .Drop(&data, MODIFIERKEYS_FLAGS(0), point, &raw mut effect)
                .expect("Drop forwards");
        }

        assert_eq!(*calls.borrow(), vec!["enter", "over", "leave", "drop"]);
        assert_eq!(
            effect, DROPEFFECT_COPY,
            "the original target's effect answer travels back through the wrapper"
        );
    }

    #[test]
    fn a_drop_without_file_paths_reports_nothing() {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let dropped = Rc::new(RefCell::new(Vec::new()));
        let wrapper = wrapper_around(&calls, &dropped);
        let data: IDataObject = EmptyDataObject.into();
        let mut effect = DROPEFFECT_NONE;

        unsafe {
            wrapper
                .Drop(
                    &data,
                    MODIFIERKEYS_FLAGS(0),
                    POINTL { x: 0, y: 0 },
                    &raw mut effect,
                )
                .expect("Drop forwards");
        }

        assert!(
            dropped.borrow().is_empty(),
            "a non-file drop never fires the path callback"
        );
        assert_eq!(
            *calls.borrow(),
            vec!["drop"],
            "the drop still reaches Chromium"
        );
    }
}

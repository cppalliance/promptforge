//! The Windows WebView2 bridge: Explorer path drops and the microphone
//! permission grant, over raw COM.
//!
//! The page cannot read real OS paths from an HTML5 drop (Chromium hides
//! them), and the app must not touch the OLE drop target either: Tauri's
//! drag-drop handler registers its own target on the WebView2 child
//! windows before Chromium can, which kills every HTML5 drag-and-drop
//! interaction inside the page (Dockview panel drags included), and once
//! Chromium does register, its target lives in the msedgewebview2 browser
//! process, so it cannot be wrapped from this process at all - a window
//! property holding another process's interface pointer is not callable
//! here.
//!
//! WebView2 has a supported channel for exactly this: on a drop the page
//! calls `chrome.webview.postMessageWithAdditionalObjects` with the DOM
//! `File` objects, and the host receives each one as an
//! [`ICoreWebView2File`] whose `Path` is the real OS path. [`attach`]
//! subscribes to those messages and feeds the paths into the
//! `promptforge:file-drop` dispatch ([`crate::drops`]) the page already
//! listens for.
//!
//! The same subscription pass installs the `PermissionRequested` handler
//! that grants the microphone (and nothing else), replacing wry's
//! `with_permission_handler` from the tao/wry shell.

use std::path::PathBuf;
use std::sync::mpsc;

use anyhow::Context as _;
use webview2_com::Microsoft::Web::WebView2::Win32::{
    COREWEBVIEW2_PERMISSION_KIND, COREWEBVIEW2_PERMISSION_KIND_MICROPHONE,
    COREWEBVIEW2_PERMISSION_STATE_ALLOW, ICoreWebView2, ICoreWebView2File,
    ICoreWebView2PermissionRequestedEventArgs, ICoreWebView2WebMessageReceivedEventArgs,
    ICoreWebView2WebMessageReceivedEventArgs2,
};
use webview2_com::{PermissionRequestedEventHandler, WebMessageReceivedEventHandler, take_pwstr};
use windows_core::{Interface as _, PWSTR};

use crate::drops::dispatch_file_drop;

/// The web message the page sends with a drop's `File` objects attached.
/// Every other message on the shared channel is ignored.
const DROP_MESSAGE: &str = "workspace-drop";

/// Subscribes the window's webview to drop messages and permission
/// requests. The message handler evals the `promptforge:file-drop`
/// dispatch into the page; the permission handler grants the microphone
/// and leaves every other kind to WebView2's default handling.
///
/// # Errors
/// Returns an error when the webview dispatch or a COM subscription fails;
/// the caller logs it and Explorer drops plus the mic grant degrade,
/// nothing else.
pub(crate) fn attach(window: &tauri::WebviewWindow) -> anyhow::Result<()> {
    let window_for_handler = window.clone();
    let (tx, rx) = mpsc::channel();
    window
        .with_webview(move |webview| {
            let _ = tx.send(attach_handlers(&webview, window_for_handler));
        })
        .context("dispatch the bridge attach to the webview thread")?;
    rx.recv()
        .context("the webview bridge attach did not answer")?
        .context("attach the WebView2 bridge")
}

/// The COM subscriptions, run on the thread that owns the webview.
fn attach_handlers(
    webview: &tauri::webview::PlatformWebview,
    window: tauri::WebviewWindow,
) -> windows_core::Result<()> {
    let controller = webview.controller();
    // SAFETY: `controller` is the live controller of this window's webview,
    // called on the webview's own thread (with_webview guarantees it), and
    // the generated out-param write is valid by construction.
    let core = unsafe { controller.CoreWebView2()? };
    attach_drop_bridge(&core, window)?;
    attach_permission_grant(&core)?;
    Ok(())
}

/// The drop half: forward the real OS paths of a `workspace-drop`
/// message's attached `File` objects into the page's grant flow.
fn attach_drop_bridge(
    core: &ICoreWebView2,
    window: tauri::WebviewWindow,
) -> windows_core::Result<()> {
    let handler = WebMessageReceivedEventHandler::create(Box::new(move |_, args| {
        let Some(args) = args else {
            return Ok(());
        };
        if message_string(&args).as_deref() == Some(DROP_MESSAGE) {
            let paths = dropped_paths(&args);
            if !paths.is_empty() {
                dispatch_file_drop(&window, &paths);
            }
        }
        Ok(())
    }));
    let mut token = 0_i64;
    // SAFETY: `core` is live on its owning thread, `handler` outlives the
    // subscription (the webview holds it), and `token` is a valid out-pointer
    // to a stack local.
    unsafe { core.add_WebMessageReceived(&handler, &raw mut token) }
}

/// The permission half: grant the microphone automatically. Every other
/// permission kind returns without a `SetState` call, keeping WebView2's
/// default handling.
fn attach_permission_grant(core: &ICoreWebView2) -> windows_core::Result<()> {
    let handler = PermissionRequestedEventHandler::create(Box::new(move |_, args| {
        let Some(args) = args else {
            return Ok(());
        };
        grant_microphone(&args)
    }));
    let mut token = 0_i64;
    // SAFETY: same preconditions as the drop subscription above.
    unsafe { core.add_PermissionRequested(&handler, &raw mut token) }
}

/// Answers one permission request: allow for the microphone, default
/// handling for everything else.
fn grant_microphone(args: &ICoreWebView2PermissionRequestedEventArgs) -> windows_core::Result<()> {
    let mut kind = COREWEBVIEW2_PERMISSION_KIND::default();
    // SAFETY: `args` is the live event argument WebView2 handed the handler,
    // and `kind` is a valid out-pointer to a stack local.
    unsafe { args.PermissionKind(&raw mut kind) }?;
    if kind == COREWEBVIEW2_PERMISSION_KIND_MICROPHONE {
        // SAFETY: `args` is live as above.
        unsafe { args.SetState(COREWEBVIEW2_PERMISSION_STATE_ALLOW) }?;
    }
    Ok(())
}

/// The message's string payload, or `None` when the message is not a
/// string (a JSON object posted by some other page code, say).
fn message_string(args: &ICoreWebView2WebMessageReceivedEventArgs) -> Option<String> {
    let mut message = PWSTR::null();
    // SAFETY: `args` is the live event argument, and `message` is a valid
    // out-pointer; on success the returned PWSTR is owned and freed by
    // take_pwstr.
    unsafe { args.TryGetWebMessageAsString(&raw mut message) }.ok()?;
    Some(take_pwstr(message))
}

/// The real OS paths of the message's attached `File` objects. Anything
/// else riding along (a message with no attachments, or attachments of
/// some other type) contributes nothing.
fn dropped_paths(args: &ICoreWebView2WebMessageReceivedEventArgs) -> Vec<PathBuf> {
    // AdditionalObjects arrived with WebView2 runtime 1.0.1518.46 (2022);
    // an older runtime fails the cast and drops degrade gracefully.
    let Ok(args) = args.cast::<ICoreWebView2WebMessageReceivedEventArgs2>() else {
        return Vec::new();
    };
    // SAFETY: `args` is live on the webview's thread; the returned
    // collection pointer is owned by the event and read only here.
    let Ok(objects) = (unsafe { args.AdditionalObjects() }) else {
        return Vec::new();
    };
    let mut count = 0_u32;
    // SAFETY: `objects` is the live collection returned above; `count` is a
    // valid out-pointer to a stack local.
    if unsafe { objects.Count(&raw mut count) }.is_err() {
        return Vec::new();
    }
    let mut paths = Vec::with_capacity(count as usize);
    for index in 0..count {
        // SAFETY: `objects` is live; `index` is below the count it reported.
        let Ok(object) = (unsafe { objects.GetValueAtIndex(index) }) else {
            continue;
        };
        let Ok(file) = object.cast::<ICoreWebView2File>() else {
            continue;
        };
        let mut path = PWSTR::null();
        // SAFETY: `file` is a live attachment; `path` is a valid
        // out-pointer, and on success the returned PWSTR is owned and freed
        // by take_pwstr.
        if unsafe { file.Path(&raw mut path) }.is_ok() {
            paths.push(PathBuf::from(take_pwstr(path)));
        }
    }
    paths
}

// The clippy allows cover code the #[implement] macro expands in tests.
#[cfg(test)]
#[allow(clippy::inline_always, clippy::ref_as_ptr)]
mod tests {
    use std::path::PathBuf;

    use webview2_com::Microsoft::Web::WebView2::Win32::{
        ICoreWebView2File, ICoreWebView2File_Impl, ICoreWebView2ObjectCollectionView,
        ICoreWebView2ObjectCollectionView_Impl, ICoreWebView2WebMessageReceivedEventArgs,
        ICoreWebView2WebMessageReceivedEventArgs_Impl, ICoreWebView2WebMessageReceivedEventArgs2,
        ICoreWebView2WebMessageReceivedEventArgs2_Impl,
    };
    use webview2_com::pwstr_from_str;
    use windows_core::{IUnknown, Interface as _, PWSTR, implement};

    use super::{DROP_MESSAGE, dropped_paths, message_string};

    /// A fake dropped file carrying one path.
    #[implement(ICoreWebView2File)]
    struct FakeFile {
        path: &'static str,
    }

    impl ICoreWebView2File_Impl for FakeFile_Impl {
        fn Path(&self, value: *mut PWSTR) -> windows_core::Result<()> {
            // SAFETY: `value` is the out-pointer the COM caller provided,
            // valid for one write.
            unsafe { *value = pwstr_from_str(self.path) };
            Ok(())
        }
    }

    /// A fake attachment collection.
    #[implement(ICoreWebView2ObjectCollectionView)]
    struct FakeObjects {
        objects: Vec<IUnknown>,
    }

    impl ICoreWebView2ObjectCollectionView_Impl for FakeObjects_Impl {
        fn Count(&self, value: *mut u32) -> windows_core::Result<()> {
            #[allow(clippy::cast_possible_truncation)]
            // SAFETY: `value` is the caller's out-pointer, valid for one write.
            unsafe {
                *value = self.objects.len() as u32;
            }
            Ok(())
        }

        fn GetValueAtIndex(&self, index: u32) -> windows_core::Result<IUnknown> {
            Ok(self.objects[index as usize].clone())
        }
    }

    /// A fake web-message event: a string message (or not) plus optional
    /// attachments, mirroring what postMessageWithAdditionalObjects sends.
    #[implement(
        ICoreWebView2WebMessageReceivedEventArgs,
        ICoreWebView2WebMessageReceivedEventArgs2
    )]
    struct FakeArgs {
        message: Option<&'static str>,
        objects: Vec<IUnknown>,
    }

    impl ICoreWebView2WebMessageReceivedEventArgs_Impl for FakeArgs_Impl {
        fn Source(&self, value: *mut PWSTR) -> windows_core::Result<()> {
            // SAFETY: `value` is the caller's out-pointer, valid for one write.
            unsafe { *value = pwstr_from_str("http://127.0.0.1/") };
            Ok(())
        }

        fn WebMessageAsJson(&self, value: *mut PWSTR) -> windows_core::Result<()> {
            // SAFETY: `value` is the caller's out-pointer, valid for one write.
            unsafe { *value = pwstr_from_str("{}") };
            Ok(())
        }

        fn TryGetWebMessageAsString(&self, value: *mut PWSTR) -> windows_core::Result<()> {
            match self.message {
                Some(message) => {
                    // SAFETY: `value` is the caller's out-pointer, valid for
                    // one write.
                    unsafe { *value = pwstr_from_str(message) };
                    Ok(())
                }
                None => Err(windows_core::Error::from_hresult(windows_core::HRESULT(
                    0x8000_000B_u32.cast_signed(), // E_BOUNDS, what WebView2 answers
                ))),
            }
        }
    }

    impl ICoreWebView2WebMessageReceivedEventArgs2_Impl for FakeArgs_Impl {
        fn AdditionalObjects(&self) -> windows_core::Result<ICoreWebView2ObjectCollectionView> {
            Ok(FakeObjects {
                objects: self.objects.clone(),
            }
            .into())
        }
    }

    fn args_with(
        message: Option<&'static str>,
        objects: Vec<IUnknown>,
    ) -> ICoreWebView2WebMessageReceivedEventArgs {
        FakeArgs { message, objects }.into()
    }

    /// A fake file attachment, as the plain IUnknown the collection holds.
    fn file_object(path: &'static str) -> IUnknown {
        let file = ICoreWebView2File::from(FakeFile { path });
        match file.cast() {
            Ok(unknown) => unknown,
            Err(error) => panic!("a file must cast to IUnknown: {error}"),
        }
    }

    #[test]
    fn the_drop_message_string_is_read_back() {
        let args = args_with(Some(DROP_MESSAGE), Vec::new());
        assert_eq!(message_string(&args).as_deref(), Some(DROP_MESSAGE));
    }

    #[test]
    fn a_non_string_message_reads_as_none() {
        let args = args_with(None, Vec::new());
        assert_eq!(message_string(&args), None);
    }

    #[test]
    fn file_attachments_become_their_os_paths() {
        let files = vec![
            file_object(r"C:\Users\Vinnie\Documents\project"),
            file_object(r"D:\src\notes.md"),
        ];
        let args = args_with(Some(DROP_MESSAGE), files);
        assert_eq!(
            dropped_paths(&args),
            vec![
                PathBuf::from(r"C:\Users\Vinnie\Documents\project"),
                PathBuf::from(r"D:\src\notes.md"),
            ]
        );
    }

    #[test]
    fn non_file_attachments_are_skipped() {
        // The collection object itself rides along as a non-file IUnknown.
        let not_a_file: IUnknown = ICoreWebView2ObjectCollectionView::from(FakeObjects {
            objects: Vec::new(),
        })
        .cast()
        .unwrap_or_else(|error| panic!("a collection must cast to IUnknown: {error}"));
        let objects = vec![not_a_file, file_object(r"C:\only\this\one.txt")];
        let args = args_with(Some(DROP_MESSAGE), objects);
        assert_eq!(
            dropped_paths(&args),
            vec![PathBuf::from(r"C:\only\this\one.txt")]
        );
    }

    #[test]
    fn a_message_with_no_attachments_yields_no_paths() {
        let args = args_with(Some(DROP_MESSAGE), Vec::new());
        assert!(dropped_paths(&args).is_empty());
    }
}

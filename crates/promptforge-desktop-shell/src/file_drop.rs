//! Explorer path drops over the WebView2 web-message channel.
//!
//! The page cannot read real OS paths from an HTML5 drop (Chromium hides
//! them), and the shell must not touch the OLE drop target either: wry's
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
//! subscribes to those messages; window.rs feeds the paths into the same
//! `promptforge:file-drop` grant flow the page already listens for.

use std::path::PathBuf;

use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2File, ICoreWebView2WebMessageReceivedEventArgs,
    ICoreWebView2WebMessageReceivedEventArgs2,
};
use webview2_com::{WebMessageReceivedEventHandler, take_pwstr};
use windows_core::{Interface as _, PWSTR};
use wry::WebViewExtWindows as _;

/// The web message the page sends with a drop's `File` objects attached.
/// Every other message on the shared channel (wry's IPC envelopes
/// included) is ignored.
const DROP_MESSAGE: &str = "workspace-drop";

/// Subscribes to the webview's web messages and calls `on_drop` with the
/// real OS paths of every `File` object posted under [`DROP_MESSAGE`].
/// The handler runs on the event loop thread for the lifetime of the
/// webview.
///
/// # Errors
/// Returns the COM error when the subscription cannot be added; the
/// caller logs it and Explorer path drops degrade, nothing else.
pub(crate) fn attach(
    webview: &wry::WebView,
    on_drop: impl Fn(Vec<PathBuf>) + 'static,
) -> windows_core::Result<()> {
    let handler = WebMessageReceivedEventHandler::create(Box::new(move |_, args| {
        let Some(args) = args else {
            return Ok(());
        };
        if message_string(&args).as_deref() == Some(DROP_MESSAGE) {
            let paths = dropped_paths(&args);
            if !paths.is_empty() {
                on_drop(paths);
            }
        }
        Ok(())
    }));
    let mut token = 0_i64;
    unsafe {
        webview
            .webview()
            .add_WebMessageReceived(&handler, &raw mut token)
    }
}

/// The message's string payload, or `None` when the message is not a
/// string (a JSON object posted by some other page code, say).
fn message_string(args: &ICoreWebView2WebMessageReceivedEventArgs) -> Option<String> {
    let mut message = PWSTR::null();
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
    let Ok(objects) = (unsafe { args.AdditionalObjects() }) else {
        return Vec::new();
    };
    let mut count = 0_u32;
    if unsafe { objects.Count(&raw mut count) }.is_err() {
        return Vec::new();
    }
    let mut paths = Vec::with_capacity(count as usize);
    for index in 0..count {
        let Ok(object) = (unsafe { objects.GetValueAtIndex(index) }) else {
            continue;
        };
        let Ok(file) = object.cast::<ICoreWebView2File>() else {
            continue;
        };
        let mut path = PWSTR::null();
        if unsafe { file.Path(&raw mut path) }.is_ok() {
            paths.push(PathBuf::from(take_pwstr(path)));
        }
    }
    paths
}

#[cfg(test)]
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
            unsafe { *value = pwstr_from_str("http://127.0.0.1/") };
            Ok(())
        }

        fn WebMessageAsJson(&self, value: *mut PWSTR) -> windows_core::Result<()> {
            unsafe { *value = pwstr_from_str("{}") };
            Ok(())
        }

        fn TryGetWebMessageAsString(&self, value: *mut PWSTR) -> windows_core::Result<()> {
            match self.message {
                Some(message) => {
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

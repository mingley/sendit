//! Explorer opens only the game. A terminal keeps its console and any redirected streams.
use std::sync::atomic::{AtomicBool, Ordering};
use windows_sys::Win32::{
    Foundation::INVALID_HANDLE_VALUE,
    Storage::FileSystem::{GetFileType, FILE_TYPE_DISK, FILE_TYPE_PIPE},
    System::Console::{
        AttachConsole, GetConsoleWindow, GetStdHandle, SetStdHandle, ATTACH_PARENT_PROCESS,
        STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    },
    UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK},
};

static REPORT_TO_WINDOW: AtomicBool = AtomicBool::new(false);

pub fn initialize() {
    // AttachConsole may replace standard handles. Keep the shell's pipes and files.
    let (console, redirected_output) = unsafe {
        let streams = [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE].map(|slot| {
            let handle = GetStdHandle(slot);
            let redirected = !handle.is_null()
                && handle != INVALID_HANDLE_VALUE
                && matches!(GetFileType(handle), FILE_TYPE_DISK | FILE_TYPE_PIPE);
            (slot, handle, redirected)
        });
        let console = AttachConsole(ATTACH_PARENT_PROCESS) != 0 || !GetConsoleWindow().is_null();
        let mut redirected_output = false;
        for (slot, handle, redirected) in streams {
            if redirected {
                if console {
                    SetStdHandle(slot, handle);
                }
                redirected_output |= slot != STD_INPUT_HANDLE;
            }
        }
        (console, redirected_output)
    };
    if !console && !redirected_output {
        REPORT_TO_WINDOW.store(true, Ordering::Release);
        let original = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |panic| {
            original(panic);
            show_error(&panic.to_string());
        }));
    }
}

pub fn show_error(message: &str) {
    if !REPORT_TO_WINDOW.swap(false, Ordering::AcqRel) {
        return;
    }
    let message: Vec<u16> = format!("Sendit could not continue.\n\n{message}")
        .encode_utf16()
        .chain([0])
        .collect();
    let title: Vec<u16> = "Sendit".encode_utf16().chain([0]).collect();
    // Both UTF-16 buffers are terminated and live for the duration of the synchronous dialog.
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            message.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}

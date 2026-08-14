//! Best-effort Windows process observation through native process APIs.

#![cfg(windows)]

use std::io;
use std::path::PathBuf;

use windows::core::PWSTR;
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::ProcessStatus::EnumProcesses;
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION,
};

const MAX_PROCESSES: usize = 32_768;

pub fn executable_paths() -> io::Result<Vec<PathBuf>> {
    let mut process_ids = vec![0_u32; MAX_PROCESSES];
    let mut bytes_returned = 0_u32;
    unsafe {
        EnumProcesses(
            process_ids.as_mut_ptr(),
            (process_ids.len() * std::mem::size_of::<u32>()) as u32,
            &mut bytes_returned,
        )
        .map_err(|error| io::Error::other(error.to_string()))?;
    }
    process_ids.truncate(bytes_returned as usize / std::mem::size_of::<u32>());

    let mut paths = Vec::new();
    for process_id in process_ids
        .into_iter()
        .filter(|process_id| *process_id != 0)
    {
        let handle =
            match unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id) } {
                Ok(handle) => handle,
                Err(_) => continue,
            };
        let mut buffer = vec![0_u16; 32_768];
        let mut length = buffer.len() as u32;
        let result = unsafe {
            QueryFullProcessImageNameW(
                handle,
                PROCESS_NAME_FORMAT(0),
                PWSTR(buffer.as_mut_ptr()),
                &mut length,
            )
        };
        unsafe {
            let _ = CloseHandle(handle);
        }
        if result.is_ok() && length > 0 {
            paths.push(PathBuf::from(String::from_utf16_lossy(
                &buffer[..length as usize],
            )));
        }
    }
    Ok(paths)
}

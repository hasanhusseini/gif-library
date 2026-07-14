use windows_sys::Win32::{
    Foundation::{GetLastError, ERROR_NOT_FOUND},
    Security::Credentials::{CredDeleteW, CRED_TYPE_GENERIC},
};

const OBSOLETE_HOSTING_CREDENTIAL: &str = "app.giflibrary.desktop.hosting";

pub fn remove_obsolete_hosting_credential() {
    let mut target: Vec<u16> = OBSOLETE_HOSTING_CREDENTIAL.encode_utf16().collect();
    target.push(0);

    let deleted = unsafe { CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) };
    if deleted != 0 {
        eprintln!("Obsolete hosting credential cleanup succeeded.");
        return;
    }

    let error = unsafe { GetLastError() };
    if error == ERROR_NOT_FOUND {
        eprintln!("Obsolete hosting credential cleanup succeeded: no credential was present.");
    } else {
        eprintln!("Obsolete hosting credential cleanup failed with Windows error code {error}.");
    }
}

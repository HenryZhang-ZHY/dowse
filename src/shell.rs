//! Explorer integration on Windows: "Add to dowse" on folders (`--add`, the
//! GUI counterpart of `code --add`), and opening `.dowse-workspace` files
//! (and `.tgrep-workspace` ones from before the rename) with a double click.
//! Written to the current user's registry only when asked for, and removed
//! the same way, along with what tgrep-gpui wrote.

use std::path::Path;

/// Where the integration stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Missing,
    /// Installed for this program.
    Installed,
    /// Installed for a program elsewhere, such as an earlier copy.
    Elsewhere,
}

/// Registry keys under `HKEY_CURRENT_USER`, each with its default value and
/// any named values.
struct Key {
    path: &'static str,
    default: String,
    values: Vec<(&'static str, String)>,
}

const FOLDER_KEY: &str = r"Software\Classes\Directory\shell\dowse";
const FOLDER_BACKGROUND_KEY: &str = r"Software\Classes\Directory\Background\shell\dowse";
const EXTENSION_KEY: &str = r"Software\Classes\.dowse-workspace";
const LEGACY_EXTENSION_KEY: &str = r"Software\Classes\.tgrep-workspace";
const PROG_ID: &str = "dowse.workspace";
const PROG_ID_KEY: &str = r"Software\Classes\dowse.workspace";

/// What tgrep-gpui, as the app was called before, wrote.
const LEGACY_KEYS: [&str; 3] = [
    r"Software\Classes\Directory\shell\tgrep-gpui",
    r"Software\Classes\Directory\Background\shell\tgrep-gpui",
    r"Software\Classes\tgrep-gpui.workspace",
];
const LEGACY_PROG_ID: &str = "tgrep-gpui.workspace";

/// The keys that make up the integration for the program at `exe`.
fn keys(exe: &Path) -> Vec<Key> {
    let exe = exe.display();
    let icon = format!("\"{exe}\",0");
    vec![
        Key {
            path: FOLDER_KEY,
            default: "Add to dowse".into(),
            values: vec![("Icon", icon.clone())],
        },
        Key {
            path: r"Software\Classes\Directory\shell\dowse\command",
            default: format!("\"{exe}\" --add \"%1\""),
            values: vec![],
        },
        Key {
            path: FOLDER_BACKGROUND_KEY,
            default: "Add to dowse".into(),
            values: vec![("Icon", icon.clone())],
        },
        Key {
            path: r"Software\Classes\Directory\Background\shell\dowse\command",
            default: format!("\"{exe}\" --add \"%V\""),
            values: vec![],
        },
        Key {
            path: EXTENSION_KEY,
            default: PROG_ID.into(),
            values: vec![],
        },
        Key {
            path: LEGACY_EXTENSION_KEY,
            default: PROG_ID.into(),
            values: vec![],
        },
        Key {
            path: PROG_ID_KEY,
            default: "dowse workspace".into(),
            values: vec![],
        },
        Key {
            path: r"Software\Classes\dowse.workspace\DefaultIcon",
            default: icon,
            values: vec![],
        },
        Key {
            path: r"Software\Classes\dowse.workspace\shell\open\command",
            default: format!("\"{exe}\" \"%1\""),
            values: vec![],
        },
    ]
}

#[cfg(windows)]
mod registry {
    use std::io;
    use std::path::Path;

    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;

    use super::*;

    pub fn state(exe: &Path) -> State {
        let user = RegKey::predef(HKEY_CURRENT_USER);
        let expected = &keys(exe)[1];
        match user
            .open_subkey(expected.path)
            .and_then(|key| key.get_value::<String, _>(""))
        {
            Ok(command) if command == expected.default => State::Installed,
            Ok(_) => State::Elsewhere,
            Err(_) => State::Missing,
        }
    }

    pub fn install(exe: &Path) -> io::Result<()> {
        let user = RegKey::predef(HKEY_CURRENT_USER);
        remove_legacy(&user)?;
        for key in keys(exe) {
            let (created, _) = user.create_subkey(key.path)?;
            created.set_value("", &key.default)?;
            for (name, value) in &key.values {
                created.set_value(name, value)?;
            }
        }
        associations_changed();
        Ok(())
    }

    pub fn uninstall() -> io::Result<()> {
        let user = RegKey::predef(HKEY_CURRENT_USER);
        for path in [FOLDER_KEY, FOLDER_BACKGROUND_KEY, PROG_ID_KEY] {
            delete(&user, path)?;
        }
        remove_legacy(&user)?;
        // The extensions may since belong to another program; leave them then.
        for extension in [EXTENSION_KEY, LEGACY_EXTENSION_KEY] {
            let ours = user
                .open_subkey(extension)
                .and_then(|key| key.get_value::<String, _>(""))
                .is_ok_and(|prog_id| prog_id == PROG_ID || prog_id == LEGACY_PROG_ID);
            if ours {
                user.delete_subkey_all(extension)?;
            }
        }
        associations_changed();
        Ok(())
    }

    /// Remove the menu entries and file type tgrep-gpui registered.
    fn remove_legacy(user: &RegKey) -> io::Result<()> {
        LEGACY_KEYS.iter().try_for_each(|path| delete(user, path))
    }

    fn delete(user: &RegKey, path: &str) -> io::Result<()> {
        match user.delete_subkey_all(path) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }

    /// Tell Explorer to pick up the new file association.
    fn associations_changed() {
        #[link(name = "shell32")]
        unsafe extern "system" {
            fn SHChangeNotify(
                event: i32,
                flags: u32,
                item1: *const std::ffi::c_void,
                item2: *const std::ffi::c_void,
            );
        }
        const SHCNE_ASSOCCHANGED: i32 = 0x0800_0000;
        const SHCNF_IDLIST: u32 = 0;
        // SAFETY: the event takes no items, so both are null.
        unsafe {
            SHChangeNotify(
                SHCNE_ASSOCCHANGED,
                SHCNF_IDLIST,
                std::ptr::null(),
                std::ptr::null(),
            );
        }
    }
}

#[cfg(windows)]
pub use registry::{install, state, uninstall};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_quote_the_program_and_the_folder() {
        let keys = keys(Path::new(r"C:\Program Files\dowse\dowse.exe"));
        let command = |path: &str| {
            keys.iter()
                .find(|key| key.path == path)
                .map(|key| key.default.clone())
                .unwrap()
        };
        assert_eq!(
            command(r"Software\Classes\Directory\shell\dowse\command"),
            r#""C:\Program Files\dowse\dowse.exe" --add "%1""#
        );
        assert_eq!(
            command(r"Software\Classes\Directory\Background\shell\dowse\command"),
            r#""C:\Program Files\dowse\dowse.exe" --add "%V""#
        );
        assert_eq!(
            command(r"Software\Classes\dowse.workspace\shell\open\command"),
            r#""C:\Program Files\dowse\dowse.exe" "%1""#
        );
        assert_eq!(command(EXTENSION_KEY), PROG_ID);
        assert_eq!(command(LEGACY_EXTENSION_KEY), PROG_ID);
    }
}

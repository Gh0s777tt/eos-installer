use anyhow::{anyhow, bail, Result};
use pkgar::{ext::EntryExt, PackageHead};
use pkgar_core::PackageSrc;
use pkg::{PackageState, PACKAGES_HEAD_DIR, PACKAGES_TOML_PATH};
use redox_installer::{try_fast_install, with_redoxfs_mount, with_whole_disk, Config, DiskOption};
use std::{
    fs,
    io::{self, Read, Write},
    os::unix::fs::{symlink, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    process,
};

// TODO: This is not the TUI a regular user would expect it does
// 1. Linux: Implement disk listing, use "dd" to write into whole disk
// 2. Allow partitioning to allow dual boot, possibly an integration with systemd-boot/grub
// 3. Prompt everything (disk password, users, preconfigured packages, import from existing img)

#[cfg(not(target_os = "redox"))]
fn disk_paths(_paths: &mut Vec<(PathBuf, u64)>) {}

#[cfg(target_os = "redox")]
fn disk_paths(paths: &mut Vec<(PathBuf, u64)>) {
    let mut schemes = Vec::new();
    match fs::read_dir("/scheme") {
        Ok(entries) => {
            for entry_res in entries {
                if let Ok(entry) = entry_res {
                    if let Ok(file_name) = entry.file_name().into_string() {
                        if file_name.starts_with("disk") {
                            schemes.push(entry.path());
                        }
                    }
                }
            }
        }
        Err(err) => {
            eprintln!("redox_installer_tui: failed to list schemes: {}", err);
        }
    }

    for scheme in schemes {
        if scheme.is_dir() {
            match fs::read_dir(&scheme) {
                Ok(entries) => {
                    for entry_res in entries {
                        if let Ok(entry) = entry_res {
                            if let Ok(file_name) = entry.file_name().into_string() {
                                if file_name.contains('p') {
                                    // Skip partitions
                                    continue;
                                }

                                if let Ok(metadata) = entry.metadata() {
                                    let size = metadata.len();
                                    if size > 0 {
                                        paths.push((entry.path(), size));
                                    }
                                }
                            }
                        }
                    }
                }
                Err(err) => {
                    eprintln!(
                        "redox_installer_tui: failed to list '{}': {}",
                        scheme.display(),
                        err
                    );
                }
            }
        }
    }
}

fn copy_file(src: &Path, dest: &Path, buf: &mut [u8]) -> Result<()> {
    // R-F22: the config install runs BEFORE this copy, and it deliberately overrides some
    // packaged files -- E-OS replaces /etc/issue with its own login banner, among 65
    // [[files]] entries. Both branches below create with create_new/symlink semantics, so
    // the first such overlap aborted the entire install with "File exists" -- measured at
    // file 12 of 13679. The config layer is the customisation and has to win, so a
    // destination that already exists is left alone instead of fought over.
    //
    // This code had never run before: package_files() failed with ENOENT ahead of it
    // (R-F21), and that failure was itself masked by the unmount error (R-F19).
    if fs::symlink_metadata(dest).is_ok() {
        return Ok(());
    }

    if let Some(parent) = dest.parent() {
        // Parent may be a symlink
        if !parent.is_symlink() {
            match fs::create_dir_all(&parent) {
                Ok(()) => (),
                Err(err) => {
                    bail!("failed to create directory {}: {}", parent.display(), err);
                }
            }
        }
    }

    let metadata = match fs::symlink_metadata(&src) {
        Ok(ok) => ok,
        Err(err) => {
            bail!("failed to read metadata of {}: {}", src.display(), err);
        }
    };

    if metadata.file_type().is_symlink() {
        let real_src = match fs::read_link(&src) {
            Ok(ok) => ok,
            Err(err) => {
                bail!("failed to read link {}: {}", src.display(), err);
            }
        };

        match symlink(&real_src, &dest) {
            Ok(()) => (),
            Err(err) => {
                bail!(
                    "failed to copy link {} ({}) to {}: {}",
                    src.display(),
                    real_src.display(),
                    dest.display(),
                    err
                );
            }
        }
    } else {
        let mut src_file = match fs::File::open(&src) {
            Ok(ok) => ok,
            Err(err) => {
                bail!("failed to open file {}: {}", src.display(), err);
            }
        };

        let mut dest_file = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(metadata.mode())
            .open(&dest)
        {
            Ok(ok) => ok,
            Err(err) => {
                bail!("failed to create file {}: {}", dest.display(), err);
            }
        };

        loop {
            let count = match src_file.read(buf) {
                Ok(ok) => ok,
                Err(err) => {
                    bail!("failed to read file {}: {}", src.display(), err);
                }
            };

            if count == 0 {
                break;
            }

            match dest_file.write_all(&buf[..count]) {
                Ok(()) => (),
                Err(err) => {
                    bail!("failed to write file {}: {}", dest.display(), err);
                }
            }
        }
    }

    Ok(())
}

fn package_files(
    root_path: &Path,
    config: &mut Config,
    files: &mut Vec<String>,
) -> Result<(), anyhow::Error> {
    //TODO: Remove packages from config where all files are located (and have valid shasum?)
    config.packages.clear();

    // R-F21: the package database moved, and this had not followed. It used to open
    // <root>/pkg/id_ed25519.pub.toml and list <root>/pkg/*.pkgar_head. Measured on a live
    // E-OS image: /pkg does not exist at all, while var/lib/packages holds 65 .pkgar_head
    // files and etc/pkg/packages.toml is present. So this failed with ENOENT before
    // copying anything -- and that failure was invisible until the unmount error stopped
    // masking the callback's result (R-F19).
    //
    // This is a rewrite rather than a path swap because the key moved too: pkg-lib keeps
    // one public key PER REMOTE in packages.toml, instead of a single file at a fixed path.
    let state = PackageState::from_sysroot(root_path)
        .map_err(|err| anyhow!("cannot read the package database: {err}"))?;

    files.push(PACKAGES_TOML_PATH.to_string());

    for (package, install) in &state.installed {
        let Some(remote) = state.pubkeys.get(&install.remote) else {
            bail!(
                "package {package} names remote {}, which has no public key in the package database",
                install.remote
            );
        };
        let rel = format!("{PACKAGES_HEAD_DIR}/{package}.pkgar_head");
        let mut pkg = PackageHead::new(&root_path.join(&rel), root_path, &remote.pkey)?;
        for entry in pkg.read_entries()? {
            files.push(entry.check_path()?.to_str().unwrap().to_string());
        }
        files.push(rel);
    }

    Ok(())
}

/// What we can honestly say about whether a disk can be unplugged.
///
/// `Unknown` is a real answer and is printed as such. Redox exposes no removability flag on
/// the disk scheme, so anything here is inferred from the INTERFACE, and an inference stated
/// as a fact is how someone ends up erasing the wrong disk with confidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Removable {
    Yes,
    No,
    Unknown,
}

impl Removable {
    fn describe(self) -> &'static str {
        match self {
            Removable::Yes => "yes (inferred from the interface)",
            Removable::No => "no (inferred from the interface)",
            Removable::Unknown => "unknown -- Redox exposes no removability flag for this interface",
        }
    }
}

/// Interface and removability, derived from the disk scheme's directory name.
///
/// Measured shape of that name, from two real installer runs:
///
///     /scheme/disk.pci-0000-00-05.0-nvme/1
///     /scheme/disk.pci-0000-00-06.0-nvme/1
///
/// Those two lines are the SAME 4 GB disk on two runs, at different PCI addresses -- which is
/// exactly why the caller must not identify a disk by its position in a list. The trailing
/// token after the last '-' names the driver, and that is all this function trusts.
///
/// Only `nvme` is backed by measurement here. Everything else returns the raw token and
/// `Unknown`, because printing "SATA, not removable" for a string this code has never seen on a
/// real machine would be inventing a fact at exactly the moment the user is deciding what to
/// destroy.
pub fn interface_of(scheme_dir: &str) -> (String, Removable) {
    let token = scheme_dir.rsplit('-').next().unwrap_or("");
    match token {
        "nvme" => ("NVMe".to_string(), Removable::No),
        "" => ("unknown".to_string(), Removable::Unknown),
        other => (other.to_string(), Removable::Unknown),
    }
}

/// The scheme directory of a disk path: `/scheme/disk.pci-…-nvme/1` -> `disk.pci-…-nvme`.
pub fn scheme_dir_of(path: &Path) -> String {
    path.parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string()
}

/// Does what the operator typed name the disk they were shown?
///
/// Compared as an EXACT string after trimming -- not as a `Path`, and that distinction is the
/// whole point. `Path` equality normalises: it was measured here that
/// `Path::new("/scheme/disk…/1/") == Path::new("/scheme/disk…/1")` is TRUE, and it collapses
/// `//` and `.` the same way. Normalisation the operator cannot see is exactly what must not
/// happen in the one prompt whose answer erases a disk. "Type it exactly as shown" has to mean
/// exactly.
///
/// On Redox the scheme path IS the identity, and two real runs produced
/// `/scheme/disk.pci-0000-00-05.0-nvme/1` and `/scheme/disk.pci-0000-00-06.0-nvme/1` for
/// different disks -- one character apart. Trimming is the only latitude given, because a
/// trailing newline is the terminal's doing and not the operator's.
pub fn confirms(typed: &str, path: &Path) -> bool {
    let typed = typed.trim();
    match path.to_str() {
        Some(p) => !typed.is_empty() && typed == p,
        // A path that is not valid UTF-8 cannot be retyped reliably, so refuse rather than
        // fall back to a looser comparison.
        None => false,
    }
}

fn choose_disk() -> PathBuf {
    let mut paths = Vec::new();
    disk_paths(&mut paths);

    if paths.is_empty() {
        eprintln!("redox_installer_tui: no RedoxFS partition found");
        eprintln!("redox_installer_tui: this tool is used to overwrite unmounted RedoxFS disk in Redox OS");
        process::exit(1);
    }

    loop {
        eprintln!();
        eprintln!("Disks found:");
        for (path, size) in paths.iter() {
            let (iface, removable) = interface_of(&scheme_dir_of(path));
            eprintln!();
            eprintln!("  \x1B[1m{}\x1B[0m", path.display());
            eprintln!("      size:       {}", redox_installer::format_bytes(*size));
            eprintln!("      interface:  {}", iface);
            eprintln!("      removable:  {}", removable.describe());
        }
        eprintln!();
        eprintln!("\x1B[1mEVERYTHING on the disk you name will be ERASED.\x1B[0m");
        eprintln!("Type its full path exactly as shown above, or 'q' to quit.");
        // Deliberately NOT a number. The list order follows PCI enumeration, which has already
        // been observed to change between runs, so a number identifies a POSITION and not a
        // device -- and the position is what moves.
        eprint!("Disk to erase: ");

        let mut line = String::new();
        match io::stdin().read_line(&mut line) {
            Ok(0) => {
                eprintln!("redox_installer_tui: failed to read line: end of input");
                process::exit(1);
            }
            Ok(_) => (),
            Err(err) => {
                eprintln!("redox_installer_tui: failed to read line: {}", err);
                process::exit(1);
            }
        }

        let typed = line.trim();
        if typed.eq_ignore_ascii_case("q") {
            eprintln!("redox_installer_tui: nothing was written; quitting at the operator's request");
            process::exit(1);
        }

        if let Some((path, _)) = paths.iter().find(|(p, _)| confirms(typed, p)) {
            break path.clone();
        }

        eprintln!();
        eprintln!("refused: {:?} does not name any disk listed above. Nothing was written.", typed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interface_is_read_from_the_scheme_name() {
        // The exact string two real runs produced.
        let (iface, rem) = interface_of("disk.pci-0000-00-05.0-nvme");
        assert_eq!(iface, "NVMe");
        assert_eq!(rem, Removable::No);
    }

    #[test]
    fn an_unmeasured_interface_is_reported_as_unknown_not_guessed() {
        // The point of the test: we must NOT claim removability for a driver this code has
        // never been run against. Saying "unknown" is the honest answer and is printed.
        let (iface, rem) = interface_of("disk.pci-0000-00-1f.2-ahci");
        assert_eq!(iface, "ahci");
        assert_eq!(rem, Removable::Unknown);
        assert!(rem.describe().contains("no removability flag"));
    }

    #[test]
    fn scheme_dir_is_the_parent_not_the_partition() {
        assert_eq!(
            scheme_dir_of(Path::new("/scheme/disk.pci-0000-00-06.0-nvme/1")),
            "disk.pci-0000-00-06.0-nvme"
        );
    }

    #[test]
    fn confirmation_needs_the_whole_path() {
        let disk = Path::new("/scheme/disk.pci-0000-00-06.0-nvme/1");
        assert!(confirms("/scheme/disk.pci-0000-00-06.0-nvme/1", disk));
        assert!(confirms("  /scheme/disk.pci-0000-00-06.0-nvme/1\n", disk));
    }

    #[test]
    fn a_near_miss_is_refused() {
        // These two differ by ONE character, and both existed in real runs. A prefix match or a
        // fuzzy compare would erase the wrong disk here.
        let disk = Path::new("/scheme/disk.pci-0000-00-06.0-nvme/1");
        assert!(!confirms("/scheme/disk.pci-0000-00-05.0-nvme/1", disk));
        assert!(!confirms("/scheme/disk.pci-0000-00-06.0-nvme", disk));
        // Refused deliberately. Rust's `Path` says this EQUALS the disk -- measured -- so the
        // comparison is on strings; a confirmation prompt must not silently normalise.
        assert!(!confirms("/scheme/disk.pci-0000-00-06.0-nvme/1/", disk));
        assert!(!confirms("/scheme//disk.pci-0000-00-06.0-nvme/1", disk));
        assert!(!confirms("1", disk));
        assert!(!confirms("", disk));
        assert!(!confirms("   ", disk));
    }
}

fn main() {
    let root_path = Path::new("/");

    let disk_path = choose_disk();

    let Ok(password_opt) = redox_installer::prompt_password(
        "redox_installer_tui: redoxfs password (empty for none)",
        "redox_installer_tui: confirm password",
    ) else {
        process::exit(1);
    };

    let instant = std::time::Instant::now();

    let bootloader_bios = {
        let path = root_path.join("usr/lib/boot/bootloader.bios");
        if path.exists() {
            match fs::read(&path) {
                Ok(ok) => ok,
                Err(err) => {
                    eprintln!(
                        "redox_installer_tui: {}: failed to read: {}",
                        path.display(),
                        err
                    );
                    process::exit(1);
                }
            }
        } else {
            Vec::new()
        }
    };

    let bootloader_efi = {
        let path = root_path.join("usr/lib/boot/bootloader.efi");
        if path.exists() {
            match fs::read(&path) {
                Ok(ok) => ok,
                Err(err) => {
                    eprintln!(
                        "redox_installer_tui: {}: failed to read: {}",
                        path.display(),
                        err
                    );
                    process::exit(1);
                }
            }
        } else {
            Vec::new()
        }
    };

    let disk_option = DiskOption {
        bootloader_bios: &bootloader_bios,
        bootloader_efi: &bootloader_efi,
        password_opt: password_opt.as_ref().map(|x| x.as_bytes()),
        efi_partition_size: None,
        skip_partitions: false, // TODO?
        // In-image install: the compile-time TARGET baked into this binary
        // matches the running system's arch.
        target: redox_installer::get_target(),
    };
    let res = with_whole_disk(&disk_path, &disk_option, |mut fs| {
        // Fast install method via filesystem clone
        let mut last_percent = 0;
        if try_fast_install(&mut fs, move |used, used_old| {
            let percent = (used * 100) / used_old;
            if percent != last_percent {
                eprint!(
                    "\r{}%: {} MB/{} MB",
                    percent,
                    used / 1000 / 1000,
                    used_old / 1000 / 1000
                );
                last_percent = percent;
            }
        })? {
            eprintln!("\rfinished installing using fast mode");
            return Ok(());
        }

        // Slow install method via file copy
        with_redoxfs_mount(fs, None, |mount_path| {
            let mut config: Config = Config::from_file(&root_path.join("filesystem.toml"))?;

            // Copy filesystem.toml, which is not packaged
            let mut files = vec!["filesystem.toml".to_string()];

            // Copy files from locally installed packages
            package_files(&root_path, &mut config, &mut files)
                // TODO: implement Error trait
                .map_err(|err| anyhow!("failed to read package files: {err}"))?;

            // Perform config install (after packages have been converted to files)
            eprintln!("configuring system");
            let cookbook: Option<&'static str> = None;
            redox_installer::install_dir(config, mount_path, cookbook)
                .map_err(|err| io::Error::other(err))?;

            // Sort and remove duplicates
            files.sort();
            files.dedup();

            // Install files
            let mut buf = vec![0; 4096 * 1024];
            for (i, name) in files.iter().enumerate() {
                eprintln!("copy {} [{}/{}]", name, i, files.len());

                let src = root_path.join(name);
                let dest = mount_path.join(name);
                copy_file(&src, &dest, &mut buf)?;
            }

            eprintln!("finished installing, unmounting filesystem");

            Ok(())
        })
    });

    match res {
        Ok(()) => {
            eprintln!(
                "redox_installer_tui: installed successfully in {:?}",
                instant.elapsed()
            );
            process::exit(0);
        }
        Err(err) => {
            eprintln!("redox_installer_tui: failed to install: {:?}", err);
            process::exit(1);
        }
    }
}

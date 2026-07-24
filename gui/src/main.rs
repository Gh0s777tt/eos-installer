use anyhow::format_err;
use cosmic::{
    app::{self, Task},
    iced::{
        self, executor,
        futures::sink::SinkExt,
        widget::{row, text_input},
        window, Alignment, Size, Subscription,
    },
    widget::{button, column, progress_bar, radio, space, text},
    Application, ApplicationExt, Core, Element,
};
use futures_channel::mpsc;
use pkgar::{ext::EntryExt, PackageHead};
use pkgar_core::PackageSrc;
use pkgar_keys::PublicKeyFile;
use redox_installer::{try_fast_install, with_redoxfs_mount, with_whole_disk, Config, DiskOption};
use std::{
    ffi::OsStr,
    fs,
    io::{self, Read, Write},
    os::unix::fs::{symlink, MetadataExt, OpenOptionsExt},
    path::Path,
    sync::Arc,
};

mod sys;
pub use sys::*;

fn main() -> iced::Result {
    let mut settings = app::Settings::default();
    settings = settings.size(Size::new(608.0, 416.0));
    settings = settings.exit_on_close(false);
    app::run::<Window>(settings, ())
}

fn copy_file(src: &Path, dest: &Path, buf: &mut [u8]) -> anyhow::Result<()> {
    if let Some(parent) = dest.parent() {
        // Parent may be a symlink
        if !parent.is_symlink() {
            match fs::create_dir_all(&parent) {
                Ok(()) => (),
                Err(err) => {
                    return Err(format_err!(
                        "failed to create directory {}: {}",
                        parent.display(),
                        err
                    ));
                }
            }
        }
    }

    let metadata = match fs::symlink_metadata(&src) {
        Ok(ok) => ok,
        Err(err) => {
            return Err(format_err!(
                "failed to read metadata of {}: {}",
                src.display(),
                err
            ));
        }
    };

    if metadata.file_type().is_symlink() {
        let real_src = match fs::read_link(&src) {
            Ok(ok) => ok,
            Err(err) => {
                return Err(format_err!(
                    "failed to read link {}: {}",
                    src.display(),
                    err
                ));
            }
        };

        match symlink(&real_src, &dest) {
            Ok(()) => (),
            Err(err) => {
                return Err(format_err!(
                    "failed to copy link {} ({}) to {}: {}",
                    src.display(),
                    real_src.display(),
                    dest.display(),
                    err
                ));
            }
        }
    } else {
        let mut src_file = match fs::File::open(&src) {
            Ok(ok) => ok,
            Err(err) => {
                return Err(format_err!(
                    "failed to open file {}: {}",
                    src.display(),
                    err
                ));
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
                return Err(format_err!(
                    "failed to create file {}: {}",
                    dest.display(),
                    err
                ));
            }
        };

        loop {
            let count = match src_file.read(buf) {
                Ok(ok) => ok,
                Err(err) => {
                    return Err(format_err!(
                        "failed to read file {}: {}",
                        src.display(),
                        err
                    ));
                }
            };

            if count == 0 {
                break;
            }

            match dest_file.write_all(&buf[..count]) {
                Ok(()) => (),
                Err(err) => {
                    return Err(format_err!(
                        "failed to write file {}: {}",
                        dest.display(),
                        err
                    ));
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

    let pkey_path = "pkg/id_ed25519.pub.toml";
    let pkey = PublicKeyFile::open(&root_path.join(pkey_path))?.pkey;
    files.push(pkey_path.to_string());

    for item_res in fs::read_dir(&root_path.join("pkg"))? {
        let item = item_res?;
        let pkg_path = item.path();
        if pkg_path.extension() == Some(OsStr::new("pkgar_head")) {
            let mut pkg = PackageHead::new(&pkg_path, &root_path, &pkey)?;
            for entry in pkg.read_entries()? {
                files.push(entry.check_path()?.to_str().unwrap().to_string());
            }
            files.push(
                pkg_path
                    .strip_prefix(root_path)
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_string(),
            );
        }
    }

    Ok(())
}

fn install<F: FnMut(Message)>(
    disk_path: String,
    password_opt: Option<String>,
    net_cfg: NetCfg,
    mut f: F,
) {
    let start = std::time::Instant::now();

    let mut progress = 0;

    macro_rules! message {
        ($($arg:tt)*) => {{
            eprintln!($($arg)*);
            f(Message::Install(
                progress,
                format!($($arg)*)
            ));
        }}
    }

    let root_path = Path::new("/scheme/file/");

    message!("Loading bootloader");
    let bootloader_bios = {
        let path = root_path.join("usr/lib/boot/bootloader.bios");
        if path.exists() {
            match fs::read(&path) {
                Ok(ok) => ok,
                Err(err) => {
                    f(Message::Error(format!(
                        "{}: failed to read: {}",
                        path.display(),
                        err
                    )));
                    return;
                }
            }
        } else {
            Vec::new()
        }
    };

    message!("Loading bootloader.efi");
    let bootloader_efi = {
        let path = root_path.join("usr/lib/boot/bootloader.efi");
        if path.exists() {
            match fs::read(&path) {
                Ok(ok) => ok,
                Err(err) => {
                    f(Message::Error(format!(
                        "{}: failed to read: {}",
                        path.display(),
                        err
                    )));
                    return;
                }
            }
        } else {
            Vec::new()
        }
    };

    message!("Formatting disk");
    let disk_option = DiskOption {
        bootloader_bios: &bootloader_bios,
        bootloader_efi: &bootloader_efi,
        password_opt: password_opt.as_ref().map(|x| x.as_bytes()),
        efi_partition_size: None,
        skip_partitions: false,
        // In-image install: the compile-time TARGET baked into this binary
        // matches the running system's arch.
        target: redox_installer::get_target(),
    };
    let res = with_whole_disk(&disk_path, &disk_option, |mut fs| -> anyhow::Result<()> {
        // Fast install method via filesystem clone
        let mut last_progress = 0;
        if try_fast_install(&mut fs, |used, used_old| {
            progress = ((used * 100) / used_old) as usize;
            if progress != last_progress {
                message!(
                    "{}%: {} MB/{} MB",
                    progress,
                    used / 1000 / 1000,
                    used_old / 1000 / 1000
                );
                last_progress = progress;
            }
        })? {
            progress = 100;
            message!("Finished installing using fast mode");
            // The fast path returns before any Config is applied, so the network
            // settings have to be written into the cloned root explicitly —
            // otherwise the pane would be silently ignored on the common
            // live-ISO install.
            message!("Applying network settings");
            return with_redoxfs_mount(fs, None, |mount_path: &Path| -> anyhow::Result<()> {
                net_cfg.write_into(mount_path)
            });
        }

        with_redoxfs_mount(fs, None, |mount_path: &Path| -> anyhow::Result<()> {
            message!("Loading filesystem.toml");
            let mut config: Config = {
                let path = root_path.join("filesystem.toml");
                match fs::read_to_string(&path) {
                    Ok(config_data) => match toml::from_str(&config_data) {
                        Ok(config) => config,
                        Err(err) => {
                            return Err(format_err!(
                                "{}: failed to decode: {}",
                                path.display(),
                                err
                            ));
                        }
                    },
                    Err(err) => {
                        return Err(format_err!("{}: failed to read: {}", path.display(), err));
                    }
                }
            };

            // Copy filesystem.toml, which is not packaged
            let mut files = vec!["filesystem.toml".to_string()];

            // Copy files from locally installed packages
            message!("Loading package files");
            if let Err(err) = package_files(&root_path, &mut config, &mut files) {
                return Err(format_err!("failed to read package files: {}", err));
            }

            // Sort and remove duplicates
            files.sort();
            files.dedup();

            // Perform config install (after packages have been converted to files)
            message!("Configuring system");
            let cookbook: Option<&'static str> = None;
            redox_installer::install_dir(config, mount_path, cookbook)
                .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;

            // Install files
            let mut buf = vec![0; 4096 * 1024];
            for (i, name) in files.iter().enumerate() {
                progress = (i * 100) / files.len();
                message!("Copy {} [{}/{}]", name, i, files.len());

                let src = root_path.join(name);
                let dest = mount_path.join(name);
                copy_file(&src, &dest, &mut buf)?;
            }

            // After install_dir and the package copy, so neither overwrites the
            // user's choice with the image's baked-in defaults.
            message!("Applying network settings");
            net_cfg.write_into(mount_path)?;

            progress = 100;
            message!("Finished installing, unmounting filesystem");
            Ok(())
        })
    });

    match res {
        Ok(()) => {
            f(Message::Success(format!(
                "Finished installing in {:?}, ready to reboot",
                start.elapsed()
            )));
        }
        Err(err) => {
            f(Message::Error(format!("Failed to install: {}", err)));
        }
    }
}

#[derive(Debug)]
enum Page {
    Sudo(String),
    Disk(Option<usize>),
    Network(NetCfg),
    Install(usize, String),
    Success(String),
    Error(String),
}

/// Network settings collected by the installer and written into the installed
/// system's `/etc/net/*`.
///
/// E-OS applies these on the first boot of the new system: `11_eos-netcfg.service`
/// runs `eos-netcfg boot`, which for `static` pushes the files onto the live
/// netcfg stack and for `dhcp` waits for the lease and mirrors it back into the
/// files. So what is chosen here is what the installed machine actually comes up
/// with — previously the installer copied the image's baked-in defaults and the
/// user had no say at all.
#[derive(Clone, Debug)]
struct NetCfg {
    dhcp: bool,
    ip: String,
    subnet: String,
    router: String,
    dns: String,
}

impl NetCfg {
    /// Pre-fill from the *running* (live/installer) system, so the form starts
    /// from values that already work on this machine rather than from blanks.
    fn from_live() -> Self {
        let read = |p: &str| {
            fs::read_to_string(p)
                .unwrap_or_default()
                .trim()
                .to_string()
        };
        let mut cfg = NetCfg {
            // The live medium is DHCP-configured by `10_dhcpd.service`, and DHCP is
            // the right default for a fresh install.
            dhcp: true,
            ip: read("/etc/net/ip"),
            subnet: read("/etc/net/ip_subnet"),
            router: read("/etc/net/ip_router"),
            dns: read("/etc/net/dns"),
        };
        if cfg.subnet.is_empty() {
            cfg.subnet = "255.255.255.0".to_string();
        }
        cfg
    }

    /// Write the settings into the freshly installed root at `mount_path`.
    ///
    /// Called on **both** install paths — the config path and the fast clone —
    /// because the fast path returns before any `Config` is applied, so a pane
    /// that only pushed `config.files` would be silently ignored on a live-ISO
    /// install (the normal case).
    fn write_into(&self, mount_path: &Path) -> anyhow::Result<()> {
        let dir = mount_path.join("etc/net");
        fs::create_dir_all(&dir)?;
        if self.dhcp {
            // Leave the address files as installed; the mode marker tells the new
            // system to lease on boot and mirror the result into them.
            fs::write(dir.join("mode"), "dhcp\n")?;
        } else {
            fs::write(dir.join("mode"), "static\n")?;
            fs::write(dir.join("ip"), format!("{}\n", self.ip))?;
            fs::write(dir.join("ip_subnet"), format!("{}\n", self.subnet))?;
            fs::write(dir.join("ip_router"), format!("{}\n", self.router))?;
            fs::write(dir.join("dns"), format!("{}\n", self.dns))?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
struct Worker {
    command_sender: std::sync::mpsc::Sender<(String, Option<String>, NetCfg)>,
    join_handle: Arc<std::thread::JoinHandle<()>>,
}

#[derive(Clone, Debug)]
enum Message {
    None,
    Worker(Worker),
    SudoInput(String),
    SudoSubmit,
    DiskChoose(usize),
    DiskConfirm(usize),
    NetDhcp(bool),
    NetIp(String),
    NetSubnet(String),
    NetRouter(String),
    NetDns(String),
    NetConfirm,
    Install(usize, String),
    Success(String),
    Exit,
    Error(String),
}

struct Window {
    core: Core,
    page: Page,
    disk_paths: Vec<(String, bool, u64)>,
    /// Disk chosen on the Disk page, remembered across the Network page.
    disk_i: Option<usize>,
    worker_opt: Option<Worker>,
}

enum State {
    Ready,
    Waiting(mpsc::UnboundedReceiver<Message>),
    Finished,
}

impl Window {
    fn worker_stream() -> impl iced::futures::Stream<Item = Message> {
        iced::stream::channel(100, |mut output: mpsc::Sender<Message>| async move {
            let mut state = State::Ready;
            loop {
                let (message, new_state) = match state {
                    State::Ready => {
                        let (command_sender, command_receiver) = std::sync::mpsc::channel();

                        let (message_sender, message_receiver) = mpsc::unbounded();

                        //TODO: kill worker thread?
                        let join_handle = std::thread::spawn(move || {
                            while let Ok((disk_path, password_opt, net_cfg)) =
                                command_receiver.recv()
                            {
                                println!("Installing to {:?}", disk_path);
                                install(disk_path, password_opt, net_cfg, |message| {
                                    message_sender.unbounded_send(message).unwrap();
                                });
                            }
                        });

                        let worker = Worker {
                            command_sender,
                            join_handle: std::sync::Arc::new(join_handle),
                        };

                        (Message::Worker(worker), State::Waiting(message_receiver))
                    }
                    State::Waiting(mut message_receiver) => {
                        use iced::futures::StreamExt;
                        match message_receiver.next().await {
                            Some(message) => (message, State::Waiting(message_receiver)),
                            None => (Message::None, State::Finished),
                        }
                    }
                    State::Finished => iced::futures::future::pending().await,
                };
                output.send(message).await.unwrap();
                state = new_state;
            }
        })
    }
}
impl Application for Window {
    type Executor = executor::Default;
    type Flags = ();
    type Message = Message;

    const APP_ID: &'static str = "org.redox-os.InstallerGui";

    fn init(core: Core, _flags: ()) -> (Self, Task<Message>) {
        //TODO: load in background
        let (page, disk_paths) = match disk_paths() {
            Ok(disk_paths) => (Page::Disk(None), disk_paths),
            Err(err) => (Page::Error(err), Vec::new()),
        };

        let mut app = Self {
            core,
            page,
            disk_paths,
            disk_i: None,
            worker_opt: None,
        };
        let task = app.set_window_title("Redox OS Installer".to_string());
        (app, task)
    }

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::None => {}
            Message::Worker(worker) => {
                self.worker_opt = Some(worker);
            }
            Message::SudoInput(password) => {
                self.page = Page::Sudo(password);
            }
            Message::SudoSubmit => {
                #[cfg(target_os = "redox")]
                if let Page::Sudo(password) = &self.page {
                    match ask_root(&password) {
                        Ok(()) => {
                            self.page = Page::Disk(None);
                        }
                        Err(err) => {
                            eprintln!("{err}");
                        }
                    }
                }
                #[cfg(target_os = "linux")]
                match ask_root() {
                    Ok(()) => {
                        if let Some(window_id) = self.core.main_window_id() {
                            return window::close(window_id);
                        }
                    }
                    Err(err) => {
                        eprintln!("{err}");
                    }
                }
            }
            Message::DiskChoose(disk_i) => {
                self.page = Page::Disk(Some(disk_i));
            }
            // Disk is chosen — collect the network settings before installing.
            Message::DiskConfirm(disk_i) => {
                self.disk_i = Some(disk_i);
                self.page = Page::Network(NetCfg::from_live());
            }
            Message::NetDhcp(dhcp) => {
                if let Page::Network(cfg) = &mut self.page {
                    cfg.dhcp = dhcp;
                }
            }
            Message::NetIp(v) => {
                if let Page::Network(cfg) = &mut self.page {
                    cfg.ip = v;
                }
            }
            Message::NetSubnet(v) => {
                if let Page::Network(cfg) = &mut self.page {
                    cfg.subnet = v;
                }
            }
            Message::NetRouter(v) => {
                if let Page::Network(cfg) = &mut self.page {
                    cfg.router = v;
                }
            }
            Message::NetDns(v) => {
                if let Page::Network(cfg) = &mut self.page {
                    cfg.dns = v;
                }
            }
            // Network settings accepted — hand disk + net to the worker.
            Message::NetConfirm => match self.disk_i.and_then(|i| self.disk_paths.get(i)) {
                Some((disk_path, _is_partition, _disk_size)) => match &self.worker_opt {
                    Some(worker) => match worker.command_sender.send((
                        disk_path.clone(),
                        None,
                        match &self.page {
                            Page::Network(cfg) => cfg.clone(),
                            _ => NetCfg::from_live(),
                        },
                    )) {
                        Ok(()) => self.page = Page::Install(0, format!("Starting install...")),
                        Err(err) => {
                            self.page = Page::Error(format!("failed to send command: {}", err));
                        }
                    },
                    None => {
                        self.page = Page::Error(format!("command sender not found"));
                    }
                },
                None => {
                    self.page = Page::Error("no disk chosen".to_string());
                }
            },
            Message::Install(progress, description) => {
                self.page = Page::Install(progress, description);
            }
            Message::Success(description) => {
                self.page = Page::Success(description);
            }
            Message::Error(err) => {
                self.page = Page::Error(err);
            }
            Message::Exit => {
                if let Some(worker) = self.worker_opt.take() {
                    drop(worker.command_sender);
                    let join_handle = Arc::try_unwrap(worker.join_handle).unwrap();
                    join_handle.join().unwrap();
                }
                if let Some(window_id) = self.core.main_window_id() {
                    return window::close(window_id);
                }
            }
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let mut widgets = Vec::new();
        match &self.page {
            Page::Sudo(password) => {
                widgets.push(text("Enter your password:").into());
                widgets.push(
                    text_input("", password)
                        .on_input(Message::SudoInput)
                        .secure(true)
                        .on_submit(Message::SudoSubmit)
                        .into(),
                );
            }
            Page::Disk(disk_i_opt) => {
                if !self.disk_paths.is_empty() {
                    if is_root() {
                        widgets.push(text("Choose a drive:").size(24).into());

                        for (disk_i, (disk_path, is_partition, disk_size)) in
                            self.disk_paths.iter().enumerate()
                        {
                            if !is_partition {
                                widgets.push(
                                    row![
                                        radio(
                                            text(disk_path),
                                            disk_i,
                                            *disk_i_opt,
                                            Message::DiskChoose
                                        ),
                                        space::horizontal(),
                                        text(redox_installer::format_bytes(*disk_size)),
                                    ]
                                    .into(),
                                );
                            }
                        }

                        if let Some(disk_i) = *disk_i_opt {
                            widgets.push(space::vertical().into());
                            widgets.push(
                                row![
                                    space::horizontal(),
                                    button::destructive("Confirm")
                                        .on_press(Message::DiskConfirm(disk_i)),
                                ]
                                .into(),
                            );
                        }
                    } else {
                        #[cfg(target_os = "linux")]
                        let page = Message::SudoSubmit;

                        #[cfg(target_os = "redox")]
                        let page = Message::SudoInput(String::new());

                        widgets.push(space::vertical().into());
                        widgets.push(
                            row![
                                text("Ask superuser permission to install into drives"),
                                space::horizontal(),
                                button::suggested("Ask root access").on_press(page),
                            ]
                            .into(),
                        );
                    }
                } else {
                    widgets.push(text("No drives found").into());
                    // TODO: expose disk.pci-*-*nvme/* */ scheme to user
                    widgets.push(text("(try to rerun with sudo)").into());
                }
            }
            Page::Network(cfg) => {
                widgets.push(text("Network configuration:").size(24).into());
                widgets.push(
                    radio(
                        text("Automatic (DHCP)"),
                        true,
                        Some(cfg.dhcp),
                        Message::NetDhcp,
                    )
                    .into(),
                );
                widgets.push(
                    radio(text("Static address"), false, Some(cfg.dhcp), Message::NetDhcp).into(),
                );
                if cfg.dhcp {
                    widgets.push(
                        text("The installed system will lease its address on boot.").into(),
                    );
                } else {
                    // Only shown for static, so the fields can never look editable
                    // while DHCP is selected.
                    widgets.push(
                        row![
                            text("IP address"),
                            space::horizontal(),
                            text_input("10.0.2.15", &cfg.ip).on_input(Message::NetIp),
                        ]
                        .into(),
                    );
                    widgets.push(
                        row![
                            text("Subnet mask"),
                            space::horizontal(),
                            text_input("255.255.255.0", &cfg.subnet).on_input(Message::NetSubnet),
                        ]
                        .into(),
                    );
                    widgets.push(
                        row![
                            text("Gateway"),
                            space::horizontal(),
                            text_input("10.0.2.2", &cfg.router).on_input(Message::NetRouter),
                        ]
                        .into(),
                    );
                    widgets.push(
                        row![
                            text("DNS server"),
                            space::horizontal(),
                            text_input("9.9.9.9", &cfg.dns).on_input(Message::NetDns),
                        ]
                        .into(),
                    );
                }
                widgets.push(space::vertical().into());
                widgets.push(
                    row![
                        space::horizontal(),
                        button::suggested("Install").on_press(Message::NetConfirm),
                    ]
                    .into(),
                );
            }
            Page::Install(progress, description) => {
                widgets.push(text("Installation progress:").size(24).into());
                widgets.push(progress_bar::determinate_linear(*progress as f32 / 100.).into());
                widgets.push(text(description).into());
            }
            Page::Success(description) => {
                widgets.push(text("Installation complete!").size(24).into());
                widgets.push(text(description).into());
                widgets.push(space::vertical().into());
                widgets.push(
                    row![
                        space::horizontal(),
                        button::standard("Exit").on_press(Message::Exit),
                    ]
                    .into(),
                );
            }
            Page::Error(err) => {
                widgets.push(text(format!("{}", err)).into());
            }
        };

        column::with_children(widgets)
            .spacing(8)
            .padding(24)
            .align_x(Alignment::Start)
            .into()
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::run(Self::worker_stream)
    }
}

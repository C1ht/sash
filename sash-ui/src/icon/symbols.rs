// SPDX-License-Identifier: Apache-2.0

//! The first-party chrome icons: Tabler Icons 3.50.0 (MIT, see NOTICE), embedded,
//! unmodified. One set only (owner, M17 E3); every glyph is drawn on a 24 x 24 grid
//! with a 2 px stroke and is a symbolic mask tinted with the theme's text colour.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Symbol {
    Power,
    Restart,
    Suspend,
    Search,
    Folder,
    Apps,
    Close,
    Maximize,
    Restore,
    Fullscreen,
    Settings,
    Clock,
    Network,
    Wifi,
    WifiOff,
    Volume,
    VolumeLow,
    Muted,
    Notifications,
    App,
}

impl Symbol {
    pub const ALL: [Symbol; 20] = [
        Symbol::Power,
        Symbol::Restart,
        Symbol::Suspend,
        Symbol::Search,
        Symbol::Folder,
        Symbol::Apps,
        Symbol::Close,
        Symbol::Maximize,
        Symbol::Restore,
        Symbol::Fullscreen,
        Symbol::Settings,
        Symbol::Clock,
        Symbol::Network,
        Symbol::Wifi,
        Symbol::WifiOff,
        Symbol::Volume,
        Symbol::VolumeLow,
        Symbol::Muted,
        Symbol::Notifications,
        Symbol::App,
    ];

    /// The Tabler file name (without ".svg").
    pub fn file(self) -> &'static str {
        match self {
            Symbol::Power => "power",
            Symbol::Restart => "refresh",
            Symbol::Suspend => "moon",
            Symbol::Search => "search",
            Symbol::Folder => "folder",
            Symbol::Apps => "layout-grid",
            Symbol::Close => "x",
            Symbol::Maximize => "window-maximize",
            Symbol::Restore => "copy",
            Symbol::Fullscreen => "arrows-maximize",
            Symbol::Settings => "settings",
            Symbol::Clock => "clock",
            Symbol::Network => "network",
            Symbol::Wifi => "wifi",
            Symbol::WifiOff => "wifi-off",
            Symbol::Volume => "volume",
            Symbol::VolumeLow => "volume-2",
            Symbol::Muted => "volume-3",
            Symbol::Notifications => "bell",
            Symbol::App => "app-window",
        }
    }

    /// What it shows, for accessible names and the contact sheet.
    pub fn label(self) -> &'static str {
        match self {
            Symbol::Power => "Power",
            Symbol::Restart => "Restart",
            Symbol::Suspend => "Suspend",
            Symbol::Search => "Search",
            Symbol::Folder => "Folder",
            Symbol::Apps => "Apps",
            Symbol::Close => "Close",
            Symbol::Maximize => "Maximize",
            Symbol::Restore => "Restore",
            Symbol::Fullscreen => "Fullscreen",
            Symbol::Settings => "Settings",
            Symbol::Clock => "Clock",
            Symbol::Network => "Network",
            Symbol::Wifi => "Wi-Fi",
            Symbol::WifiOff => "Wi-Fi off",
            Symbol::Volume => "Volume",
            Symbol::VolumeLow => "Volume low",
            Symbol::Muted => "Muted",
            Symbol::Notifications => "Notifications",
            Symbol::App => "Application",
        }
    }

    pub fn svg(self) -> &'static [u8] {
        match self {
            Symbol::Power => include_bytes!("../../icons/tabler/power.svg"),
            Symbol::Restart => include_bytes!("../../icons/tabler/refresh.svg"),
            Symbol::Suspend => include_bytes!("../../icons/tabler/moon.svg"),
            Symbol::Search => include_bytes!("../../icons/tabler/search.svg"),
            Symbol::Folder => include_bytes!("../../icons/tabler/folder.svg"),
            Symbol::Apps => include_bytes!("../../icons/tabler/layout-grid.svg"),
            Symbol::Close => include_bytes!("../../icons/tabler/x.svg"),
            Symbol::Maximize => include_bytes!("../../icons/tabler/window-maximize.svg"),
            Symbol::Restore => include_bytes!("../../icons/tabler/copy.svg"),
            Symbol::Fullscreen => include_bytes!("../../icons/tabler/arrows-maximize.svg"),
            Symbol::Settings => include_bytes!("../../icons/tabler/settings.svg"),
            Symbol::Clock => include_bytes!("../../icons/tabler/clock.svg"),
            Symbol::Network => include_bytes!("../../icons/tabler/network.svg"),
            Symbol::Wifi => include_bytes!("../../icons/tabler/wifi.svg"),
            Symbol::WifiOff => include_bytes!("../../icons/tabler/wifi-off.svg"),
            Symbol::Volume => include_bytes!("../../icons/tabler/volume.svg"),
            Symbol::VolumeLow => include_bytes!("../../icons/tabler/volume-2.svg"),
            Symbol::Muted => include_bytes!("../../icons/tabler/volume-3.svg"),
            Symbol::Notifications => include_bytes!("../../icons/tabler/bell.svg"),
            Symbol::App => include_bytes!("../../icons/tabler/app-window.svg"),
        }
    }
}

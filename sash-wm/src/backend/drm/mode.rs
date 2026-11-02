// SPDX-License-Identifier: Apache-2.0

//! Which connector and modes the DRM backend tries. No settings.
//!
//! - Connector: the first connected one (by connector id) that has modes.
//! - Native resolution: with a known physical size, the largest mode whose aspect ratio
//!   is within 5% of the panel's; otherwise (or if none matches) the preferred mode's
//!   resolution, else the largest. Interlaced modes are ignored.
//! - Candidates, in the order the backend tests them (an atomic TEST_ONLY commit each):
//!   the native resolution from its highest exact refresh rate down, then each lower
//!   resolution of the same aspect, largest first, from its highest refresh down.
//! - Debug builds: `--mode WxH@R` asks for one mode instead.

/// A connector as the selection sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectorInfo {
    pub id: u32,
    pub connected: bool,
    /// Physical size in millimetres as the kernel reports it.
    pub size_mm: (u32, u32),
    pub modes: Vec<ModeInfo>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModeInfo {
    pub width: u16,
    pub height: u16,
    /// Exact refresh rate in mHz: pixel clock / (htotal x vtotal).
    pub refresh_mhz: u32,
    pub preferred: bool,
    pub interlaced: bool,
}

impl ModeInfo {
    fn pixels(&self) -> u32 {
        self.width as u32 * self.height as u32
    }

    fn resolution(&self) -> (u16, u16) {
        (self.width, self.height)
    }
}

/// Aspect ratios within this fraction of each other count as the same.
const ASPECT_TOLERANCE: f64 = 0.05;

/// Physical sizes that mean "unknown": none at all, and the placeholders some EDIDs
/// carry (16:9 written as 160x90 mm, or as 16x9 in centimetre units).
pub fn physical_size_known((w, h): (u32, u32)) -> bool {
    w > 0 && h > 0 && (w, h) != (160, 90) && (w, h) != (16, 9)
}

fn same_aspect(a: f64, b: f64) -> bool {
    (a / b - 1.0).abs() <= ASPECT_TOLERANCE
}

fn aspect(width: u32, height: u32) -> f64 {
    width as f64 / height.max(1) as f64
}

/// The connector to drive, as an index into `connectors`.
pub fn choose_connector(connectors: &[ConnectorInfo]) -> Option<usize> {
    connectors
        .iter()
        .enumerate()
        .filter(|(_, c)| c.connected && !c.modes.is_empty())
        .min_by_key(|(_, c)| c.id)
        .map(|(index, _)| index)
}

/// The native resolution of `connector` (see the module documentation).
pub fn native_resolution(connector: &ConnectorInfo) -> Option<(u16, u16)> {
    let usable = || connector.modes.iter().filter(|m| !m.interlaced);
    // The largest by pixel count; on a tie the first in the kernel's order.
    let largest = |modes: &mut dyn Iterator<Item = &ModeInfo>| {
        modes
            .fold(None::<&ModeInfo>, |best, m| match best {
                Some(b) if b.pixels() >= m.pixels() => Some(b),
                _ => Some(m),
            })
            .map(ModeInfo::resolution)
    };
    if physical_size_known(connector.size_mm) {
        let panel = aspect(connector.size_mm.0, connector.size_mm.1);
        let matching = largest(
            &mut usable().filter(|m| same_aspect(aspect(m.width as u32, m.height as u32), panel)),
        );
        if matching.is_some() {
            return matching;
        }
    }
    usable()
        .find(|m| m.preferred)
        .map(ModeInfo::resolution)
        .or_else(|| largest(&mut usable()))
}

/// The modes to test, in order, as indexes into `connector.modes`: the native
/// resolution from the highest refresh rate down, then each lower resolution of the
/// same aspect (largest first), each from its highest refresh rate down.
pub fn candidates(connector: &ConnectorInfo) -> Vec<usize> {
    let Some(native) = native_resolution(connector) else {
        return Vec::new();
    };
    let native_pixels = native.0 as u32 * native.1 as u32;
    let native_aspect = aspect(native.0 as u32, native.1 as u32);
    // Resolutions in the order they are tried.
    let mut resolutions = vec![native];
    let mut lower: Vec<(u16, u16)> = Vec::new();
    for m in connector.modes.iter().filter(|m| !m.interlaced) {
        let resolution = m.resolution();
        if m.pixels() < native_pixels
            && same_aspect(aspect(m.width as u32, m.height as u32), native_aspect)
            && !lower.contains(&resolution)
        {
            lower.push(resolution);
        }
    }
    // Largest first; a stable sort keeps the kernel's order between equal sizes.
    lower.sort_by_key(|&(w, h)| std::cmp::Reverse(w as u32 * h as u32));
    resolutions.extend(lower);
    let mut order = Vec::new();
    for resolution in resolutions {
        let mut at: Vec<usize> = (0..connector.modes.len())
            .filter(|&i| {
                let m = &connector.modes[i];
                !m.interlaced && m.resolution() == resolution
            })
            .collect();
        at.sort_by_key(|&i| std::cmp::Reverse(connector.modes[i].refresh_mhz));
        order.extend(at);
    }
    order
}

/// A mode asked for with `--mode WxH@R` (debug builds).
#[cfg(debug_assertions)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModeRequest {
    pub width: u16,
    pub height: u16,
    /// Refresh rate in Hz, compared with the exact rate rounded.
    pub refresh: u32,
}

#[cfg(debug_assertions)]
impl ModeRequest {
    /// Parses "WxH@R", e.g. "5120x1440@60".
    pub fn parse(text: &str) -> Option<Self> {
        let (size, refresh) = text.split_once('@')?;
        let (width, height) = size.split_once('x')?;
        let request = Self {
            width: width.parse().ok()?,
            height: height.parse().ok()?,
            refresh: refresh.parse().ok()?,
        };
        (request.width > 0 && request.height > 0 && request.refresh > 0).then_some(request)
    }
}

/// The modes matching `request` (size, and the exact refresh rate rounded to Hz),
/// highest refresh first: the only candidates when `--mode` is given.
#[cfg(debug_assertions)]
pub fn requested(connector: &ConnectorInfo, request: ModeRequest) -> Vec<usize> {
    let mut found: Vec<usize> = (0..connector.modes.len())
        .filter(|&i| {
            let m = &connector.modes[i];
            !m.interlaced
                && (m.width, m.height) == (request.width, request.height)
                && (m.refresh_mhz + 500) / 1000 == request.refresh
        })
        .collect();
    found.sort_by_key(|&i| std::cmp::Reverse(connector.modes[i].refresh_mhz));
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(width: u16, height: u16, refresh_mhz: u32, preferred: bool) -> ModeInfo {
        ModeInfo {
            width,
            height,
            refresh_mhz,
            preferred,
            interlaced: false,
        }
    }

    fn connector(id: u32, size_mm: (u32, u32), modes: Vec<ModeInfo>) -> ConnectorInfo {
        ConnectorInfo {
            id,
            connected: true,
            size_mm,
            modes,
        }
    }

    /// The owner's monitor (probe after M9 stage B, `--all-modes`): 1190x340 mm (32:9)
    /// on HDMI-A-1, 26 modes in the kernel's order; 3840x1080@120 is preferred.
    fn owners_monitor() -> ConnectorInfo {
        connector(
            828,
            (1190, 340),
            vec![
                mode(3840, 1080, 120_000, true),
                mode(3840, 2160, 59_940, false),
                mode(3840, 2160, 50_000, false),
                mode(5120, 1440, 240_000, false),
                mode(5120, 1440, 120_000, false),
                mode(5120, 1440, 60_000, false),
                mode(3840, 1080, 240_000, false),
                mode(3840, 1080, 60_000, false),
                mode(2560, 1440, 239_970, false),
                mode(2560, 1440, 119_998, false),
                mode(2560, 1440, 59_951, false),
                mode(1920, 1080, 239_997, false),
                mode(1920, 1080, 119_879, false),
                mode(1920, 1080, 59_939, false),
                mode(1920, 1080, 50_000, false),
                mode(1680, 1050, 59_954, false),
                mode(1600, 900, 60_000, false),
                mode(1280, 1024, 60_020, false),
                mode(1440, 900, 59_887, false),
                mode(1280, 800, 59_810, false),
                mode(1280, 720, 60_000, false),
                mode(1280, 720, 59_943, false),
                mode(800, 600, 60_317, false),
                mode(720, 480, 59_940, false),
                mode(640, 480, 75_000, false),
                mode(640, 480, 59_940, false),
            ],
        )
    }

    fn picked(c: &ConnectorInfo) -> Vec<(u16, u16, u32)> {
        candidates(c)
            .into_iter()
            .map(|i| (c.modes[i].width, c.modes[i].height, c.modes[i].refresh_mhz))
            .collect()
    }

    #[test]
    fn the_owners_monitor_gets_its_native_resolution_at_240_hz() {
        let monitor = owners_monitor();
        assert_eq!(native_resolution(&monitor), Some((5120, 1440)));
        // 5120x1440 from 240 Hz down, then the next lower 32:9 resolution (3840x1080)
        // from 240 Hz down. 3840x2160 has more pixels but is 16:9: never a candidate.
        assert_eq!(
            picked(&monitor),
            vec![
                (5120, 1440, 240_000),
                (5120, 1440, 120_000),
                (5120, 1440, 60_000),
                (3840, 1080, 240_000),
                (3840, 1080, 120_000),
                (3840, 1080, 60_000),
            ]
        );
    }

    #[test]
    fn unknown_or_placeholder_physical_sizes_fall_back_to_the_preferred_mode() {
        assert!(physical_size_known((1190, 340)));
        for unknown in [(0, 0), (0, 340), (1190, 0), (160, 90), (16, 9)] {
            assert!(!physical_size_known(unknown), "{unknown:?}");
            let mut monitor = owners_monitor();
            monitor.size_mm = unknown;
            // The preferred resolution (3840x1080) from its highest refresh down, then
            // the lower 32:9 resolutions (none here).
            assert_eq!(
                native_resolution(&monitor),
                Some((3840, 1080)),
                "{unknown:?}"
            );
            assert_eq!(
                picked(&monitor),
                vec![
                    (3840, 1080, 240_000),
                    (3840, 1080, 120_000),
                    (3840, 1080, 60_000),
                ],
                "{unknown:?}"
            );
        }
        // A real 16:9 panel of about 160x90 mm would also count as unknown; with a
        // preferred mode at its native resolution the result is the same.
        let small = connector(
            1,
            (160, 90),
            vec![
                mode(1920, 1080, 60_000, true),
                mode(1280, 720, 60_000, false),
            ],
        );
        assert_eq!(native_resolution(&small), Some((1920, 1080)));
    }

    #[test]
    fn a_vm_without_size_or_preferred_mode_takes_the_largest() {
        let vm = connector(
            1,
            (0, 0),
            vec![
                mode(1024, 768, 60_000, false),
                mode(1280, 800, 74_934, false),
                mode(1280, 800, 59_810, false),
                mode(800, 500, 60_000, false),
            ],
        );
        assert_eq!(
            picked(&vm),
            vec![(1280, 800, 74_934), (1280, 800, 59_810), (800, 500, 60_000)]
        );
    }

    #[test]
    fn no_mode_matches_the_physical_aspect() {
        // A 4:3 projector size with only 16:9 modes: the preferred mode's resolution.
        let projector = connector(
            1,
            (400, 300),
            vec![
                mode(1920, 1080, 60_000, false),
                mode(1280, 720, 60_000, true),
            ],
        );
        assert_eq!(native_resolution(&projector), Some((1280, 720)));
    }

    #[test]
    fn interlaced_modes_are_ignored() {
        let mut interlaced = mode(1920, 1080, 60_000, true);
        interlaced.interlaced = true;
        let c = connector(
            1,
            (530, 300),
            vec![interlaced, mode(1280, 720, 60_000, false)],
        );
        assert_eq!(picked(&c), vec![(1280, 720, 60_000)]);
        let only = connector(1, (530, 300), vec![interlaced]);
        assert_eq!(candidates(&only), Vec::<usize>::new());
    }

    #[test]
    fn equal_refresh_rates_keep_the_kernel_order() {
        let c = connector(
            1,
            (600, 340),
            vec![
                mode(2560, 1440, 60_000, false),
                mode(2560, 1440, 144_000, false),
                mode(2560, 1440, 60_000, true),
            ],
        );
        assert_eq!(candidates(&c), vec![1, 0, 2]);
    }

    #[test]
    fn the_lowest_connector_id_among_connected_ones() {
        let mut off = connector(40, (0, 0), vec![mode(800, 600, 60_000, true)]);
        off.connected = false;
        let connectors = [
            connector(50, (0, 0), vec![mode(1920, 1080, 60_000, true)]),
            off,
            connector(45, (0, 0), vec![mode(1280, 1024, 60_000, true)]),
            connector(30, (0, 0), vec![]),
        ];
        assert_eq!(choose_connector(&connectors), Some(2));
        assert_eq!(choose_connector(&[]), None);
    }

    #[cfg(debug_assertions)]
    #[test]
    fn a_requested_mode() {
        assert_eq!(
            ModeRequest::parse("5120x1440@60"),
            Some(ModeRequest {
                width: 5120,
                height: 1440,
                refresh: 60
            })
        );
        for bad in [
            "",
            "5120x1440",
            "5120@60",
            "x1440@60",
            "0x1440@60",
            "5120x1440@0",
            "ax1@2",
        ] {
            assert_eq!(ModeRequest::parse(bad), None, "{bad}");
        }
        let monitor = owners_monitor();
        let at = |w, h, r| {
            requested(
                &monitor,
                ModeRequest {
                    width: w,
                    height: h,
                    refresh: r,
                },
            )
        };
        assert_eq!(at(5120, 1440, 60), vec![5]);
        // 59.94 Hz rounds to 60.
        assert_eq!(at(3840, 2160, 60), vec![1]);
        assert_eq!(at(1280, 720, 60), vec![20, 21]);
        assert_eq!(at(5120, 1440, 144), Vec::<usize>::new());
    }
}

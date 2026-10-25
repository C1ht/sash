// SPDX-License-Identifier: Apache-2.0

//! Which connector and mode the DRM backend uses: the first connected connector (by
//! connector id) that has modes, at its preferred mode. Without a preferred mode, the
//! largest one, then the highest refresh rate. No settings.

/// A connector as the selection sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectorInfo {
    pub id: u32,
    pub connected: bool,
    pub modes: Vec<ModeInfo>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModeInfo {
    pub width: u16,
    pub height: u16,
    /// Refresh rate in Hz, as the kernel rounds it.
    pub refresh: u32,
    pub preferred: bool,
}

/// The chosen connector and mode, as indexes into `connectors` and its `modes`.
pub fn choose(connectors: &[ConnectorInfo]) -> Option<(usize, usize)> {
    let (index, connector) = connectors
        .iter()
        .enumerate()
        .filter(|(_, c)| c.connected && !c.modes.is_empty())
        .min_by_key(|(_, c)| c.id)?;
    let mode = connector
        .modes
        .iter()
        .position(|m| m.preferred)
        .or_else(|| {
            let best = connector.modes.iter().enumerate().max_by_key(|(i, m)| {
                (
                    m.width as u32 * m.height as u32,
                    m.refresh,
                    std::cmp::Reverse(*i),
                )
            });
            best.map(|(i, _)| i)
        })?;
    Some((index, mode))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(width: u16, height: u16, refresh: u32, preferred: bool) -> ModeInfo {
        ModeInfo {
            width,
            height,
            refresh,
            preferred,
        }
    }

    fn connector(id: u32, connected: bool, modes: Vec<ModeInfo>) -> ConnectorInfo {
        ConnectorInfo {
            id,
            connected,
            modes,
        }
    }

    #[test]
    fn the_preferred_mode_of_the_connected_connector() {
        // The owner's machine (M9 stage A probe): HDMI-A-1 connected, 3440x1440@60
        // preferred even though 3840x2160 and 100 Hz exist; three DP connectors off.
        let connectors = [
            connector(
                828,
                true,
                vec![
                    mode(3840, 2160, 60, false),
                    mode(3440, 1440, 100, false),
                    mode(3440, 1440, 60, true),
                ],
            ),
            connector(833, false, vec![]),
            connector(837, false, vec![]),
        ];
        assert_eq!(choose(&connectors), Some((0, 2)));
    }

    #[test]
    fn without_a_preferred_mode_the_largest_then_the_fastest() {
        let connectors = [connector(
            1,
            true,
            vec![
                mode(1920, 1080, 60, false),
                mode(2560, 1440, 60, false),
                mode(2560, 1440, 144, false),
                mode(1280, 720, 240, false),
            ],
        )];
        assert_eq!(choose(&connectors), Some((0, 2)));
    }

    #[test]
    fn the_first_preferred_mode_wins() {
        let connectors = [connector(
            1,
            true,
            vec![mode(1024, 768, 60, true), mode(1920, 1080, 60, true)],
        )];
        assert_eq!(choose(&connectors), Some((0, 0)));
    }

    #[test]
    fn the_lowest_connector_id_among_connected_ones() {
        let connectors = [
            connector(50, true, vec![mode(1920, 1080, 60, true)]),
            connector(40, false, vec![mode(800, 600, 60, true)]),
            connector(45, true, vec![mode(1280, 1024, 60, true)]),
            connector(30, true, vec![]),
        ];
        assert_eq!(choose(&connectors), Some((2, 0)));
    }

    #[test]
    fn nothing_connected() {
        assert_eq!(choose(&[]), None);
        let connectors = [
            connector(1, false, vec![mode(800, 600, 60, true)]),
            connector(2, true, vec![]),
        ];
        assert_eq!(choose(&connectors), None);
    }
}

// SPDX-License-Identifier: Apache-2.0

//! xdg popups: placement against the output, grabs, dismissal and re-constraining.
//!
//! A popup is placed relative to its parent's window geometry by the positioner
//! (Smithay's protocol math: flip, then slide, then resize, per axis), with the
//! output as the constraint area. It is drawn directly above its parent window,
//! unclipped, and follows it. Reactive popups are placed again when their parent
//! changes; every popup is placed again when the output changes.
//!
//! Grabs: a grabbed chain (menus) has keyboard focus. A click outside the chain
//! dismisses it, innermost first, and is not delivered; focus returns to the parent.
//! Smithay's `PopupManager` and `PopupGrab` keep the trees and the grab order; the
//! input rules are sash-wm's (see `seat.rs`).

use smithay::{
    desktop::{
        PopupGrab, PopupKind, PopupManager, PopupUngrabStrategy, Window, find_popup_root_surface,
        get_popup_toplevel_coords,
    },
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_wm_base,
        wayland_server::{Resource, protocol::wl_surface::WlSurface},
    },
    utils::{Logical, Point, Rectangle, Serial},
    wayland::{
        compositor,
        shell::xdg::{PopupSurface, PositionerState, XDG_POPUP_ROLE},
    },
};

use crate::{
    clients::ClientState,
    layout::{Rect, WindowId},
    state::Sash,
};

/// Where a popup goes, relative to its parent's window geometry: the positioner's
/// rectangle, adjusted by its constraint adjustments to fit `bounds`. `parent_origin`
/// is the parent's window geometry origin in the coordinates of `bounds`.
pub fn place(
    positioner: PositionerState,
    parent_origin: Point<i32, Logical>,
    bounds: Rect,
) -> Rectangle<i32, Logical> {
    let target = Rectangle::new(
        (bounds.x - parent_origin.x, bounds.y - parent_origin.y).into(),
        (bounds.w, bounds.h).into(),
    );
    positioner.get_unconstrained_geometry(target)
}

/// The popups of a window with the global position of each popup's buffer, topmost
/// (innermost) first. `rect` is where the window's geometry is shown.
pub fn popups_of(window: &Window, rect: Rect) -> Vec<(PopupKind, Point<i32, Logical>)> {
    let Some(toplevel) = window.toplevel() else {
        return Vec::new();
    };
    PopupManager::popups_for_surface(toplevel.wl_surface())
        .map(|(popup, offset)| {
            let origin = Point::from((rect.x, rect.y)) + offset - popup.geometry().loc;
            (popup, origin)
        })
        .collect()
}

#[derive(Default)]
pub struct Popups {
    pub manager: PopupManager,
    /// The grabbed chain, if any (Smithay's bookkeeping; its root is a toplevel).
    grab: Option<PopupGrab<Sash>>,
    /// The latest pointer button press and key press delivered to a client, with the
    /// surface that got it: a grab must carry one of these serials.
    last_button: Option<(Serial, WlSurface)>,
    last_key: Option<(Serial, WlSurface)>,
}

impl Popups {
    pub fn note_button_press(&mut self, serial: Serial, surface: WlSurface) {
        self.last_button = Some((serial, surface));
    }

    pub fn note_key_press(&mut self, serial: Serial, surface: WlSurface) {
        self.last_key = Some((serial, surface));
    }

    /// Whether `serial` is the latest press delivered to `surface`'s client.
    fn valid_grab_serial(&self, serial: Serial, surface: &WlSurface) -> bool {
        [&self.last_button, &self.last_key]
            .into_iter()
            .flatten()
            .any(|(s, to)| *s == serial && to.id().same_client_as(&surface.id()))
    }

    /// The root toplevel of the active grab.
    pub fn grab_root(&self) -> Option<WlSurface> {
        let grab = self.grab.as_ref().filter(|g| !g.has_ended())?;
        let topmost = grab.current_grab()?;
        if compositor::get_role(&topmost) == Some(XDG_POPUP_ROLE) {
            let popup = self.manager.find_popup(&topmost)?;
            find_popup_root_surface(&popup).ok()
        } else {
            Some(topmost)
        }
    }

    /// The surface with keyboard focus while a grab is active: its topmost popup.
    fn grab_focus(&self) -> Option<WlSurface> {
        self.grab
            .as_ref()
            .filter(|g| !g.has_ended())
            .and_then(|g| g.current_grab())
    }

    /// Whether `surface` (or the popup it is a subsurface of) is a popup of the
    /// grabbed chain's toplevel.
    pub fn in_grab_chain(&self, surface: &WlSurface) -> bool {
        let Some(root) = self.grab_root() else {
            return false;
        };
        let top = std::iter::successors(Some(surface.clone()), compositor::get_parent)
            .last()
            .expect("starts with surface");
        self.manager
            .find_popup(&top)
            .and_then(|p| find_popup_root_surface(&p).ok())
            .is_some_and(|r| r == root)
    }
}

impl Sash {
    /// Global origin of a window's geometry, if it is mapped and shown.
    fn shown_origin(&self, root: &WlSurface) -> Option<Point<i32, Logical>> {
        let managed = self.windows.by_surface(root).filter(|m| m.mapped)?;
        let rect = self.windows.layout.shown_rect(managed.id)?;
        Some((rect.x, rect.y).into())
    }

    /// The popup's rectangle relative to its parent for this positioner, constrained
    /// to the output. None if its toplevel is not mapped.
    fn popup_geometry(
        &self,
        popup: &PopupKind,
        positioner: PositionerState,
    ) -> Option<Rectangle<i32, Logical>> {
        let root = find_popup_root_surface(popup).ok()?;
        let parent_origin = self.shown_origin(&root)? + get_popup_toplevel_coords(popup);
        Some(place(positioner, parent_origin, self.windows.layout.area()))
    }

    pub fn new_popup(&mut self, surface: PopupSurface, positioner: PositionerState) {
        let kind = PopupKind::Xdg(surface.clone());
        // A parent popup that was dismissed (no longer tracked): dismiss this one too.
        if let Some(parent) = surface.get_parent_surface()
            && compositor::get_role(&parent) == Some(XDG_POPUP_ROLE)
            && self.popups.manager.find_popup(&parent).is_none()
        {
            tracing::debug!("popup on a dismissed parent popup dismissed");
            surface.send_popup_done();
            return;
        }
        let Some(geometry) = self.popup_geometry(&kind, positioner) else {
            // The parent is not mapped (owner decision: dismiss at once).
            tracing::debug!("popup on an unmapped parent dismissed");
            surface.send_popup_done();
            return;
        };
        surface.with_pending_state(|state| {
            state.positioner = positioner;
            state.geometry = geometry;
        });
        if let Err(err) = self.popups.manager.track_popup(kind) {
            tracing::warn!(?err, "could not track popup");
        }
    }

    /// A commit on a popup surface: the initial one gets the initial configure.
    pub fn popup_committed(&mut self, surface: &WlSurface) {
        if let Some(PopupKind::Xdg(popup)) = self.popups.manager.find_popup(surface)
            && !popup.is_initial_configure_sent()
            && let Err(err) = popup.send_configure()
        {
            tracing::warn!(?err, "popup configure failed");
        }
        self.request_redraw();
    }

    pub fn reposition_popup(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        let Some(geometry) = self.popup_geometry(&PopupKind::Xdg(surface.clone()), positioner)
        else {
            return;
        };
        surface.with_pending_state(|state| {
            state.positioner = positioner;
            state.geometry = geometry;
        });
        surface.send_repositioned(token);
    }

    /// xdg_popup.grab. An invalid serial (not the latest press delivered to this
    /// client) dismisses the popup; the other protocol rules are Smithay's
    /// (not_the_topmost_popup, a dismissed parent, a grab after mapping).
    pub fn grab_popup(&mut self, surface: PopupSurface, serial: Serial) {
        let kind = PopupKind::Xdg(surface.clone());
        let Ok(root) = find_popup_root_surface(&kind) else {
            return;
        };
        if self
            .popups
            .manager
            .find_popup(surface.wl_surface())
            .is_none()
        {
            // Already dismissed (see `new_popup`).
            return;
        }
        if !self.popups.valid_grab_serial(serial, surface.wl_surface()) {
            tracing::debug!(?serial, "popup grab with an invalid serial: dismissed");
            surface.send_popup_done();
            return;
        }
        if self.popups.grab_root().is_some_and(|r| r != root) {
            self.dismiss_grab();
        }
        let seat = self.seat.clone();
        match self.popups.manager.grab_popup(root, kind, &seat, serial) {
            Ok(grab) => {
                self.popups.grab = Some(grab);
                self.sync_keyboard_focus();
            }
            Err(err) => tracing::debug!(%err, "popup grab refused"),
        }
    }

    /// Dismisses the grabbed chain, innermost first, and gives focus back.
    pub fn dismiss_grab(&mut self) {
        if let Some(mut grab) = self.popups.grab.take() {
            grab.ungrab(PopupUngrabStrategy::All);
            self.sync_keyboard_focus();
        }
    }

    /// Dismisses every popup of a window (grabbed or not), innermost first.
    pub fn dismiss_popups(&mut self, id: WindowId) {
        let Some(root) = self
            .windows
            .by_id(id)
            .and_then(|m| m.window.toplevel().map(|t| t.wl_surface().clone()))
        else {
            return;
        };
        if self.popups.grab_root().as_ref() == Some(&root) {
            self.dismiss_grab();
        }
        let children: Vec<PopupKind> = PopupManager::popups_for_surface(&root)
            .map(|(popup, _)| popup)
            .filter(|popup| match popup {
                PopupKind::Xdg(p) => p.get_parent_surface().as_ref() == Some(&root),
                PopupKind::InputMethod(_) => false,
            })
            .collect();
        for popup in children {
            let _ = PopupManager::dismiss_popup(&root, &popup);
        }
        self.request_redraw();
    }

    /// The xdg_popup object is gone. Destroying a popup that still has a child popup
    /// is a protocol error.
    pub fn popup_destroyed(&mut self, surface: &PopupSurface) {
        let has_child =
            self.protocols.xdg_shell.popup_surfaces().iter().any(|p| {
                p.alive() && p.get_parent_surface().as_ref() == Some(surface.wl_surface())
            });
        if has_child {
            self.post_not_the_topmost_popup(surface.wl_surface());
        }
        self.popups.manager.cleanup();
        if self.popups.grab.as_ref().is_some_and(|g| g.has_ended()) {
            self.popups.grab = None;
        }
        self.sync_keyboard_focus();
        self.request_redraw();
    }

    fn post_not_the_topmost_popup(&self, surface: &WlSurface) {
        let wm_base = surface
            .client()
            .and_then(|c| c.get_data::<ClientState>().and_then(|d| d.xdg_wm_base()));
        match wm_base {
            Some(wm_base) => wm_base.post_error(
                xdg_wm_base::Error::NotTheTopmostPopup,
                "a popup was destroyed before its child popup",
            ),
            None => tracing::warn!("popup destroyed out of order; no xdg_wm_base to report it"),
        }
    }

    /// Places reactive popups again after their parents or the output changed.
    /// Non-reactive popups keep their position relative to the parent; after an
    /// output change (`output_changed`) they are dismissed instead, since they may
    /// not be configured again (owner decision).
    pub fn reconstrain_popups(&mut self, output_changed: bool) {
        let roots: Vec<WlSurface> = self
            .windows
            .all()
            .filter(|m| m.mapped)
            .filter_map(|m| m.window.toplevel().map(|t| t.wl_surface().clone()))
            .collect();
        for root in roots {
            // Parents before children.
            let popups: Vec<PopupKind> = PopupManager::popups_for_surface(&root)
                .map(|(popup, _)| popup)
                .collect();
            let mut dismiss = Vec::new();
            for popup in popups.into_iter().rev() {
                let PopupKind::Xdg(surface) = &popup else {
                    continue;
                };
                if !surface.is_initial_configure_sent() {
                    continue;
                }
                let (positioner, current) =
                    surface.with_pending_state(|state| (state.positioner, state.geometry));
                if !positioner.reactive {
                    if output_changed {
                        dismiss.push(popup);
                    }
                    continue;
                }
                let Some(geometry) = self.popup_geometry(&popup, positioner) else {
                    continue;
                };
                if geometry != current {
                    surface.with_pending_state(|state| state.geometry = geometry);
                    if let Err(err) = surface.send_pending_configure() {
                        tracing::warn!(?err, "popup configure failed");
                    }
                }
            }
            if dismiss.is_empty() {
                continue;
            }
            if dismiss
                .iter()
                .any(|popup| self.popups.in_grab_chain(popup.wl_surface()))
            {
                self.dismiss_grab();
            }
            for popup in dismiss {
                // Children of an already dismissed popup are gone with it.
                if self.popups.manager.find_popup(popup.wl_surface()).is_some() {
                    let _ = PopupManager::dismiss_popup(&root, &popup);
                }
            }
            self.request_redraw();
        }
    }

    /// Keyboard focus while a grab is active: the grab's topmost popup, as long as the
    /// layout's focused window is the grab's toplevel; otherwise the grab ends.
    pub fn popup_focus(&mut self, focused: Option<&WlSurface>) -> Option<WlSurface> {
        let root = self.popups.grab_root()?;
        if focused != Some(&root) {
            self.dismiss_grab_quietly();
            return None;
        }
        self.popups.grab_focus()
    }

    /// Ends the grab without touching keyboard focus (the caller sets it).
    fn dismiss_grab_quietly(&mut self) {
        if let Some(mut grab) = self.popups.grab.take() {
            grab.ungrab(PopupUngrabStrategy::All);
        }
    }
}

#[cfg(test)]
mod tests;

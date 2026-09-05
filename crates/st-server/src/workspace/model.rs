//! The Workspace domain model — `docs/plan/03-server.md` §3.
//!
//! This module is pure: no tokio, no I/O, no PTYs. Every mutation is a method
//! that either succeeds and describes what the caller must do about the
//! processes (kill this Surface, re-seed that Session) or fails with the
//! [`ErrorBody`] the control plane will send back verbatim. Revision bumping
//! lives one level up in [`crate::workspace::actor`], so that exactly one
//! revision is spent per request even when a request touches several
//! aggregates.
//!
//! Ids: Sessions and Tabs are numbered from one shared counter owned here.
//! Surface ids come from the [`SurfaceSpawner`](crate::workspace::SurfaceSpawner)
//! because the process and the id are allocated together.

use std::collections::BTreeMap;

use st_proto::control::{
    ErrorBody, ErrorCode, Layout, Revision, Selection, SplitAxis, SplitRatio, SurfaceMeta,
    SurfaceState, ViewState,
};
use st_proto::{SessionId, SurfaceId, TabId, WorkspaceSnapshot};

/// The name of the Session the daemon creates when it has nothing to restore
/// (grilling Q48).
pub const DEFAULT_SESSION_NAME: &str = "Default";

/// Whether a Surface's process is still alive (`03-server.md` §3).
///
/// The wire form is [`SurfaceState`]; this one additionally carries the pid,
/// which `server.status` and the supervisor want but the protocol does not
/// expose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SurfaceStatus {
    /// The process is running under `pid`, when the spawner reported one.
    Running {
        /// The child's pid, when known.
        pid: Option<u32>,
    },
    /// The process ended.
    Exited {
        /// Exit code, when it exited normally.
        code: Option<i32>,
        /// Signal name (e.g. `"SIGKILL"`), when it was killed.
        signal: Option<String>,
    },
}

impl SurfaceStatus {
    /// `true` while the process is alive.
    #[must_use]
    pub fn is_running(&self) -> bool {
        matches!(self, SurfaceStatus::Running { .. })
    }

    /// The protocol projection of this status.
    #[must_use]
    pub fn to_wire(&self) -> SurfaceState {
        match self {
            SurfaceStatus::Running { .. } => SurfaceState::Running,
            SurfaceStatus::Exited { code, signal } => SurfaceState::Exited {
                code: *code,
                signal: signal.clone(),
            },
        }
    }
}

/// Server-side Surface metadata. The grid itself lives in the data plane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Surface {
    /// Surface id, allocated by the spawner.
    pub id: SurfaceId,
    /// Title as set by the program (OSC 0/2), else the shell's file name.
    pub title: String,
    /// Title set through `surface.rename`; wins in the UI.
    pub user_title: Option<String>,
    /// Working directory, as last observed.
    pub cwd: Option<String>,
    /// argv the process was spawned with; persisted so a re-seed matches.
    pub shell: Vec<String>,
    /// Grid width.
    pub cols: u16,
    /// Grid height.
    pub rows: u16,
    /// Whether something other than the shell is in the foreground (Q48).
    pub has_foreground_child: bool,
    /// Running or exited.
    pub status: SurfaceStatus,
    /// Scroll offset and selection (Q17), edited from either plane (Q43).
    pub view: ViewState,
    /// Grilling Q42: an auto-seeded shell that has never received input and
    /// has no child process. Pristine Surfaces count as zero for idle exit.
    pub pristine: bool,
}

impl Surface {
    /// The protocol projection of this Surface.
    #[must_use]
    pub fn to_meta(&self) -> SurfaceMeta {
        SurfaceMeta {
            id: self.id,
            title: self.title.clone(),
            user_title: self.user_title.clone(),
            cwd: self.cwd.clone(),
            cols: self.cols,
            rows: self.rows,
            has_foreground_child: self.has_foreground_child,
            state: self.status.to_wire(),
            view_state: self.view.clone(),
        }
    }
}

/// A Tab: a tree of Splits whose leaves are Panes (ADR 0009).
///
/// `surface` is always the first leaf of `layout`; every mutation of the
/// layout goes through [`Tab::set_layout`] so the two cannot drift.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tab {
    /// Tab id.
    pub id: TabId,
    /// The first Pane's Surface.
    pub surface: SurfaceId,
    /// The layout tree.
    pub layout: Layout,
}

impl Tab {
    /// A single-Pane Tab.
    #[must_use]
    pub fn leaf(id: TabId, surface: SurfaceId) -> Self {
        Self {
            id,
            surface,
            layout: Layout::leaf(surface),
        }
    }

    /// A Tab with the given layout; `surface` follows its first leaf.
    #[must_use]
    pub fn with_layout(id: TabId, layout: Layout) -> Self {
        Self {
            id,
            surface: layout.first_leaf(),
            layout,
        }
    }

    /// Replaces the layout, keeping `surface` at the first leaf.
    pub fn set_layout(&mut self, layout: Layout) {
        self.surface = layout.first_leaf();
        self.layout = layout;
    }

    /// The Surfaces of every Pane, in tree order.
    #[must_use]
    pub fn surfaces(&self) -> Vec<SurfaceId> {
        self.layout.leaves()
    }

    /// The protocol projection of this Tab.
    #[must_use]
    pub fn to_wire(&self) -> st_proto::Tab {
        st_proto::Tab {
            id: self.id,
            surface: self.surface,
            layout: self.layout.clone(),
        }
    }
}

/// A named, ordered group of Tabs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    /// Session id.
    pub id: SessionId,
    /// Display name.
    pub name: String,
    /// The Tab to show when this Session becomes active; persisted (Q48).
    pub active_tab: Option<TabId>,
    /// Tabs, in display order.
    pub tabs: Vec<Tab>,
}

impl Session {
    /// The protocol projection of this Session.
    #[must_use]
    pub fn to_wire(&self) -> st_proto::Session {
        st_proto::Session {
            id: self.id,
            name: self.name.clone(),
            active_tab: self.active_tab,
            tabs: self.tabs.iter().map(Tab::to_wire).collect(),
        }
    }
}

/// What closing a Tab implies for the processes and for the Workspace shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabClosed {
    /// The Surfaces whose processes must be killed (grilling Q21): one per
    /// Pane, in tree order.
    pub surfaces: Vec<SurfaceId>,
    /// Set when the Tab was the last one in its Session, which therefore went
    /// away too.
    pub session_deleted: Option<SessionId>,
    /// `true` when no Session is left, so the caller must re-seed one
    /// (grilling Q21) with [`Workspace::seed_default_session`].
    pub needs_reseed: bool,
}

/// What closing one Pane implies (ADR 0009).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneClosed {
    /// The Surface whose process must be killed.
    pub surface: SurfaceId,
    /// Set when the Pane was the Tab's last one, so the Tab closed with it
    /// (the same cascade as `tab.close`).
    pub tab_closed: Option<TabClosed>,
}

/// The Workspace: Sessions → Tabs → Surfaces, plus the revision counter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    /// Monotonically increasing document revision; bumped by
    /// [`Workspace::bump_revision`] once per successful request.
    revision: Revision,
    /// Shared counter behind [`SessionId`] and [`TabId`].
    next_id: u32,
    /// The Session clients should show.
    pub active_session: SessionId,
    /// Sessions, in display order.
    pub sessions: Vec<Session>,
    /// Every Surface, attached to a Tab or detached (`surface.create`).
    pub surfaces: BTreeMap<SurfaceId, Surface>,
}

impl Default for Workspace {
    fn default() -> Self {
        Self::new()
    }
}

fn not_found(what: &str, id: impl std::fmt::Display) -> ErrorBody {
    ErrorBody::new(ErrorCode::NotFound, format!("{what} {id} does not exist"))
}

impl Workspace {
    /// An empty Workspace with no Sessions. The caller is expected to either
    /// re-seed from `workspace.json` or call [`Workspace::seed_default_session`].
    #[must_use]
    pub fn new() -> Self {
        Self {
            revision: 0,
            next_id: 1,
            active_session: SessionId::ZERO,
            sessions: Vec::new(),
            surfaces: BTreeMap::new(),
        }
    }

    /// The current revision.
    #[must_use]
    pub fn revision(&self) -> Revision {
        self.revision
    }

    /// Bumps and returns the revision. Called once per successful mutation.
    pub fn bump_revision(&mut self) -> Revision {
        self.revision += 1;
        self.revision
    }

    /// The next value the shared Session/Tab id counter will hand out.
    #[must_use]
    pub fn next_id(&self) -> u32 {
        self.next_id
    }

    /// Restores the id counter after a reload so ids are never reused within
    /// the persisted history either.
    pub fn set_next_id(&mut self, next_id: u32) {
        self.next_id = self.next_id.max(next_id);
    }

    fn alloc_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Allocates a fresh [`SessionId`].
    pub fn alloc_session_id(&mut self) -> SessionId {
        SessionId(self.alloc_id())
    }

    /// Allocates a fresh [`TabId`].
    pub fn alloc_tab_id(&mut self) -> TabId {
        TabId(self.alloc_id())
    }

    /// Checks an `if_revision` guard (§3.2): stale ⇒ [`ErrorCode::Conflict`].
    pub fn check_revision(&self, if_revision: Option<Revision>) -> Result<(), ErrorBody> {
        match if_revision {
            Some(want) if want != self.revision => Err(ErrorBody {
                code: ErrorCode::Conflict,
                message: format!(
                    "workspace is at revision {}, request carried if_revision {want}",
                    self.revision
                ),
                data: Some(serde_json::json!({ "revision": self.revision })),
            }),
            _ => Ok(()),
        }
    }

    // ------------------------------------------------------------ lookups

    /// Finds a Session, or the `not_found` error the client should see.
    pub fn session(&self, id: SessionId) -> Result<&Session, ErrorBody> {
        self.sessions
            .iter()
            .find(|s| s.id == id)
            .ok_or_else(|| not_found("session", id))
    }

    /// Mutable [`Workspace::session`].
    pub fn session_mut(&mut self, id: SessionId) -> Result<&mut Session, ErrorBody> {
        self.sessions
            .iter_mut()
            .find(|s| s.id == id)
            .ok_or_else(|| not_found("session", id))
    }

    /// The index of the Session holding `tab`, and the Tab's index in it.
    pub fn locate_tab(&self, tab: TabId) -> Result<(usize, usize), ErrorBody> {
        for (si, session) in self.sessions.iter().enumerate() {
            if let Some(ti) = session.tabs.iter().position(|t| t.id == tab) {
                return Ok((si, ti));
            }
        }
        Err(not_found("tab", tab))
    }

    /// Finds a Surface, or the `not_found` error the client should see.
    pub fn surface(&self, id: SurfaceId) -> Result<&Surface, ErrorBody> {
        self.surfaces
            .get(&id)
            .ok_or_else(|| not_found("surface", id))
    }

    /// Mutable [`Workspace::surface`].
    pub fn surface_mut(&mut self, id: SurfaceId) -> Result<&mut Surface, ErrorBody> {
        self.surfaces
            .get_mut(&id)
            .ok_or_else(|| not_found("surface", id))
    }

    /// `true` when some Pane of some Tab shows `surface`.
    #[must_use]
    pub fn surface_is_attached(&self, surface: SurfaceId) -> bool {
        self.sessions
            .iter()
            .any(|s| s.tabs.iter().any(|t| t.layout.contains(surface)))
    }

    /// Finds a Tab, or the `not_found` error the client should see.
    pub fn tab(&self, tab: TabId) -> Result<&Tab, ErrorBody> {
        let (si, ti) = self.locate_tab(tab)?;
        Ok(&self.sessions[si].tabs[ti])
    }

    /// Mutable [`Workspace::tab`].
    pub fn tab_mut(&mut self, tab: TabId) -> Result<&mut Tab, ErrorBody> {
        let (si, ti) = self.locate_tab(tab)?;
        Ok(&mut self.sessions[si].tabs[ti])
    }

    /// Surfaces whose process is still running.
    #[must_use]
    pub fn live_surfaces(&self) -> usize {
        self.surfaces
            .values()
            .filter(|s| s.status.is_running())
            .count()
    }

    /// Live Surfaces that are not pristine (grilling Q42): the ones that make
    /// the daemon worth keeping alive.
    #[must_use]
    pub fn busy_surfaces(&self) -> usize {
        self.surfaces
            .values()
            .filter(|s| s.status.is_running() && !s.pristine)
            .count()
    }

    // ------------------------------------------------------------ projections

    /// The protocol document.
    #[must_use]
    pub fn document(&self) -> st_proto::Workspace {
        st_proto::Workspace {
            revision: self.revision,
            active_session: self.active_session,
            sessions: self.sessions.iter().map(Session::to_wire).collect(),
        }
    }

    /// The document plus metadata for every Surface, i.e. the result of
    /// `workspace.get` and the body of `ev.workspace`.
    #[must_use]
    pub fn snapshot(&self) -> WorkspaceSnapshot {
        WorkspaceSnapshot {
            workspace: self.document(),
            surfaces: self.surfaces.values().map(Surface::to_meta).collect(),
        }
    }

    // ------------------------------------------------------------ sessions

    /// Appends a Session and returns its id.
    pub fn create_session(&mut self, name: String) -> SessionId {
        let id = self.alloc_session_id();
        self.sessions.push(Session {
            id,
            name,
            active_tab: None,
            tabs: Vec::new(),
        });
        if self.sessions.len() == 1 {
            self.active_session = id;
        }
        id
    }

    /// Restores a Session with ids chosen by the caller (used by the loader).
    pub fn insert_session(&mut self, session: Session) {
        self.set_next_id(session.id.get() + 1);
        for tab in &session.tabs {
            self.set_next_id(tab.id.get() + 1);
        }
        if self.sessions.is_empty() {
            self.active_session = session.id;
        }
        self.sessions.push(session);
    }

    /// Renames a Session.
    pub fn rename_session(&mut self, id: SessionId, name: String) -> Result<(), ErrorBody> {
        self.session_mut(id)?.name = name;
        Ok(())
    }

    /// Deletes a Session, returning the Surfaces whose processes must die
    /// (grilling Q21). `needs_reseed` in the result means no Session is left.
    pub fn delete_session(&mut self, id: SessionId) -> Result<(Vec<SurfaceId>, bool), ErrorBody> {
        let index = self
            .sessions
            .iter()
            .position(|s| s.id == id)
            .ok_or_else(|| not_found("session", id))?;
        let session = self.sessions.remove(index);
        let surfaces: Vec<SurfaceId> = session.tabs.iter().flat_map(Tab::surfaces).collect();
        for surface in &surfaces {
            self.surfaces.remove(surface);
        }
        self.repair_active_session();
        Ok((surfaces, self.sessions.is_empty()))
    }

    /// Switches the active Session.
    pub fn set_active_session(&mut self, id: SessionId) -> Result<(), ErrorBody> {
        self.session(id)?;
        self.active_session = id;
        Ok(())
    }

    fn repair_active_session(&mut self) {
        if !self.sessions.iter().any(|s| s.id == self.active_session) {
            self.active_session = self.sessions.first().map_or(SessionId::ZERO, |s| s.id);
        }
    }

    // ------------------------------------------------------------ tabs

    /// Inserts a Tab showing `surface` into `session` at `index` (appended
    /// when `index` is `None` or past the end) and makes it active.
    pub fn insert_tab(
        &mut self,
        session: SessionId,
        index: Option<u32>,
        tab: TabId,
        surface: SurfaceId,
    ) -> Result<(), ErrorBody> {
        let session = self.session_mut(session)?;
        let at = index.map_or(session.tabs.len(), |i| (i as usize).min(session.tabs.len()));
        session.tabs.insert(at, Tab::leaf(tab, surface));
        session.active_tab = Some(tab);
        Ok(())
    }

    /// Closes a Tab — every Pane of it — applying the grilling-Q21 cascade.
    pub fn close_tab(&mut self, tab: TabId) -> Result<TabClosed, ErrorBody> {
        let (si, ti) = self.locate_tab(tab)?;
        let removed = self.sessions[si].tabs.remove(ti);
        let surfaces = removed.surfaces();
        for surface in &surfaces {
            self.surfaces.remove(surface);
        }

        let session_id = self.sessions[si].id;
        let mut session_deleted = None;
        if self.sessions[si].tabs.is_empty() {
            self.sessions.remove(si);
            session_deleted = Some(session_id);
        } else {
            let fallback = ti.min(self.sessions[si].tabs.len() - 1);
            let session = &mut self.sessions[si];
            if session.active_tab == Some(tab) {
                session.active_tab = Some(session.tabs[fallback].id);
            }
        }
        self.repair_active_session();

        Ok(TabClosed {
            surfaces,
            session_deleted,
            needs_reseed: self.sessions.is_empty(),
        })
    }

    // ------------------------------------------------------------ panes (ADR 0009)

    /// Splits the Pane of `tab` showing `pane`, putting the already spawned
    /// `new` Surface in the new Pane (to the right for `Row`, below for
    /// `Column`) at an even ratio.
    pub fn split_pane(
        &mut self,
        tab: TabId,
        pane: SurfaceId,
        axis: SplitAxis,
        new: SurfaceId,
    ) -> Result<(), ErrorBody> {
        let entry = self.tab_mut(tab)?;
        let mut layout = entry.layout.clone();
        if !layout.split_leaf(pane, axis, new) {
            return Err(ErrorBody::new(
                ErrorCode::NotFound,
                format!("tab {tab} has no pane showing surface {pane}"),
            ));
        }
        entry.set_layout(layout);
        Ok(())
    }

    /// Closes the Pane of `tab` showing `pane`, collapsing its Split into
    /// the sibling. Closing the last Pane closes the Tab ([`Workspace::close_tab`]).
    pub fn close_pane(&mut self, tab: TabId, pane: SurfaceId) -> Result<PaneClosed, ErrorBody> {
        let entry = self.tab_mut(tab)?;
        match entry.layout.without_leaf(pane) {
            None => Err(ErrorBody::new(
                ErrorCode::NotFound,
                format!("tab {tab} has no pane showing surface {pane}"),
            )),
            Some(None) => {
                let closed = self.close_tab(tab)?;
                Ok(PaneClosed {
                    surface: pane,
                    tab_closed: Some(closed),
                })
            }
            Some(Some(rest)) => {
                entry.set_layout(rest);
                self.surfaces.remove(&pane);
                Ok(PaneClosed {
                    surface: pane,
                    tab_closed: None,
                })
            }
        }
    }

    /// Sets the ratio of the Split at `path` in `tab`, clamped to the range
    /// `tab.set_ratio` allows.
    pub fn set_split_ratio(
        &mut self,
        tab: TabId,
        path: &[u8],
        ratio: SplitRatio,
    ) -> Result<(), ErrorBody> {
        let entry = self.tab_mut(tab)?;
        if !entry.layout.set_ratio(path, ratio.clamped()) {
            return Err(ErrorBody::new(
                ErrorCode::NotFound,
                format!("tab {tab} has no split at path {path:?}"),
            ));
        }
        Ok(())
    }

    /// Moves a Tab within its own Session.
    pub fn reorder_tab(&mut self, tab: TabId, index: u32) -> Result<(), ErrorBody> {
        let (si, ti) = self.locate_tab(tab)?;
        let tabs = &mut self.sessions[si].tabs;
        let entry = tabs.remove(ti);
        let at = (index as usize).min(tabs.len());
        tabs.insert(at, entry);
        Ok(())
    }

    /// Moves a Tab to another Session, applying the same Q21 cascade to the
    /// Session it leaves.
    pub fn move_tab(
        &mut self,
        tab: TabId,
        to_session: SessionId,
        index: Option<u32>,
    ) -> Result<Option<SessionId>, ErrorBody> {
        let (si, ti) = self.locate_tab(tab)?;
        let target = self
            .sessions
            .iter()
            .position(|s| s.id == to_session)
            .ok_or_else(|| not_found("session", to_session))?;
        if target == si {
            self.reorder_tab(tab, index.unwrap_or(u32::MAX))?;
            return Ok(None);
        }

        let entry = self.sessions[si].tabs.remove(ti);
        let mut source_deleted = None;
        if self.sessions[si].tabs.is_empty() {
            source_deleted = Some(self.sessions[si].id);
        } else if self.sessions[si].active_tab == Some(tab) {
            let fallback = ti.min(self.sessions[si].tabs.len() - 1);
            self.sessions[si].active_tab = Some(self.sessions[si].tabs[fallback].id);
        }

        let dest = &mut self.sessions[target];
        let at = index.map_or(dest.tabs.len(), |i| (i as usize).min(dest.tabs.len()));
        dest.tabs.insert(at, entry);
        dest.active_tab = Some(tab);

        if let Some(gone) = source_deleted {
            self.sessions.retain(|s| s.id != gone);
        }
        self.repair_active_session();
        Ok(source_deleted)
    }

    /// Makes `tab` its Session's active one, and that Session active (Q48).
    pub fn set_active_tab(&mut self, tab: TabId) -> Result<(), ErrorBody> {
        let (si, _) = self.locate_tab(tab)?;
        self.sessions[si].active_tab = Some(tab);
        self.active_session = self.sessions[si].id;
        Ok(())
    }

    // ------------------------------------------------------------ surfaces

    /// Registers a freshly spawned Surface.
    pub fn insert_surface(&mut self, surface: Surface) {
        self.surfaces.insert(surface.id, surface);
    }

    /// Sets or clears a Surface's user title.
    pub fn rename_surface(
        &mut self,
        id: SurfaceId,
        user_title: Option<String>,
    ) -> Result<(), ErrorBody> {
        self.surface_mut(id)?.user_title = user_title;
        Ok(())
    }

    /// Applies a `view.set` edit. Both fields are tri-state: `None` leaves the
    /// value alone, `Some(None)` clears the selection (§3.3).
    pub fn set_view_state(
        &mut self,
        id: SurfaceId,
        scroll_offset: Option<u32>,
        selection: Option<Option<Selection>>,
    ) -> Result<(), ErrorBody> {
        let surface = self.surface_mut(id)?;
        if let Some(offset) = scroll_offset {
            surface.view.scroll_offset = offset;
        }
        if let Some(selection) = selection {
            surface.view.selection = selection;
        }
        Ok(())
    }

    /// Records a process exit. Returns `false` when the Surface is unknown or
    /// had already exited, so the caller can skip the event.
    pub fn mark_exited(
        &mut self,
        id: SurfaceId,
        code: Option<i32>,
        signal: Option<String>,
    ) -> bool {
        let Some(surface) = self.surfaces.get_mut(&id) else {
            return false;
        };
        if !surface.status.is_running() {
            return false;
        }
        surface.status = SurfaceStatus::Exited { code, signal };
        surface.pristine = false;
        surface.has_foreground_child = false;
        true
    }

    /// Grilling Q42: any input, or any foreground child, ends pristineness.
    pub fn mark_dirty(&mut self, id: SurfaceId) {
        if let Some(surface) = self.surfaces.get_mut(&id) {
            surface.pristine = false;
        }
    }

    /// Seeds the Workspace with one Session named `Default` holding one Tab
    /// (grilling Q21 and Q48). The Surface must already have been spawned.
    pub fn seed_default_session(&mut self, surface: SurfaceId) -> (SessionId, TabId) {
        let session = self.create_session(DEFAULT_SESSION_NAME.to_string());
        let tab = self.alloc_tab_id();
        self.insert_tab(session, None, tab, surface)
            .expect("the session was just created");
        self.active_session = session;
        (session, tab)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn surface(id: u32) -> Surface {
        Surface {
            id: SurfaceId(id),
            title: "zsh".into(),
            user_title: None,
            cwd: Some("/home/sonny".into()),
            shell: vec!["/bin/zsh".into()],
            cols: 80,
            rows: 24,
            has_foreground_child: false,
            status: SurfaceStatus::Running { pid: Some(1234) },
            view: ViewState::default(),
            pristine: true,
        }
    }

    fn seeded() -> Workspace {
        let mut ws = Workspace::new();
        ws.insert_surface(surface(101));
        ws.seed_default_session(SurfaceId(101));
        ws
    }

    #[test]
    fn seeding_makes_one_default_session_with_one_tab() {
        let ws = seeded();
        assert_eq!(ws.sessions.len(), 1);
        assert_eq!(ws.sessions[0].name, DEFAULT_SESSION_NAME);
        assert_eq!(ws.sessions[0].tabs.len(), 1);
        assert_eq!(ws.active_session, ws.sessions[0].id);
        assert_eq!(ws.sessions[0].active_tab, Some(ws.sessions[0].tabs[0].id));
    }

    #[test]
    fn ids_are_never_reused() {
        let mut ws = seeded();
        let a = ws.alloc_tab_id();
        let b = ws.alloc_session_id();
        assert_ne!(a.get(), b.get());
        assert!(b.get() > a.get());
    }

    #[test]
    fn closing_the_last_tab_deletes_the_session_and_asks_for_a_reseed() {
        let mut ws = seeded();
        let tab = ws.sessions[0].tabs[0].id;
        let closed = ws.close_tab(tab).unwrap();
        assert_eq!(closed.surfaces, vec![SurfaceId(101)]);
        assert!(closed.session_deleted.is_some());
        assert!(closed.needs_reseed);
        assert!(ws.sessions.is_empty());
        assert!(ws.surfaces.is_empty());
    }

    #[test]
    fn closing_a_tab_moves_the_active_tab_along() {
        let mut ws = seeded();
        ws.insert_surface(surface(102));
        let second = ws.alloc_tab_id();
        let session = ws.sessions[0].id;
        ws.insert_tab(session, None, second, SurfaceId(102))
            .unwrap();
        assert_eq!(ws.sessions[0].active_tab, Some(second));

        let closed = ws.close_tab(second).unwrap();
        assert!(!closed.needs_reseed);
        assert_eq!(ws.sessions[0].tabs.len(), 1);
        assert_eq!(ws.sessions[0].active_tab, Some(ws.sessions[0].tabs[0].id));
    }

    #[test]
    fn moving_the_last_tab_out_deletes_the_source_session() {
        let mut ws = seeded();
        let other = ws.create_session("work".into());
        let tab = ws.sessions[0].tabs[0].id;
        let source = ws.sessions[0].id;

        let deleted = ws.move_tab(tab, other, None).unwrap();
        assert_eq!(deleted, Some(source));
        assert_eq!(ws.sessions.len(), 1);
        assert_eq!(ws.sessions[0].id, other);
        assert_eq!(ws.active_session, other);
    }

    #[test]
    fn reorder_clamps_out_of_range_indices() {
        let mut ws = seeded();
        ws.insert_surface(surface(102));
        let second = ws.alloc_tab_id();
        let session = ws.sessions[0].id;
        ws.insert_tab(session, None, second, SurfaceId(102))
            .unwrap();

        ws.reorder_tab(second, 999).unwrap();
        assert_eq!(ws.sessions[0].tabs[1].id, second);
        ws.reorder_tab(second, 0).unwrap();
        assert_eq!(ws.sessions[0].tabs[0].id, second);
    }

    #[test]
    fn stale_if_revision_is_a_conflict() {
        let mut ws = seeded();
        ws.bump_revision();
        assert!(ws.check_revision(Some(1)).is_ok());
        assert!(ws.check_revision(None).is_ok());
        let err = ws.check_revision(Some(0)).unwrap_err();
        assert_eq!(err.code, ErrorCode::Conflict);
        assert_eq!(err.data.unwrap()["revision"], 1);
    }

    #[test]
    fn unknown_ids_are_not_found() {
        let ws = seeded();
        assert_eq!(
            ws.session(SessionId(77)).unwrap_err().code,
            ErrorCode::NotFound
        );
        assert_eq!(
            ws.locate_tab(TabId(77)).unwrap_err().code,
            ErrorCode::NotFound
        );
        assert_eq!(
            ws.surface(SurfaceId(77)).unwrap_err().code,
            ErrorCode::NotFound
        );
    }

    #[test]
    fn pristine_surfaces_do_not_count_as_busy() {
        let mut ws = seeded();
        assert_eq!(ws.live_surfaces(), 1);
        assert_eq!(ws.busy_surfaces(), 0, "grilling Q42");
        ws.mark_dirty(SurfaceId(101));
        assert_eq!(ws.busy_surfaces(), 1);
        assert!(ws.mark_exited(SurfaceId(101), Some(0), None));
        assert_eq!(ws.live_surfaces(), 0);
        assert!(
            !ws.mark_exited(SurfaceId(101), Some(0), None),
            "exit is idempotent"
        );
    }

    #[test]
    fn view_state_edits_are_tri_state() {
        let mut ws = seeded();
        ws.set_view_state(SurfaceId(101), Some(7), None).unwrap();
        assert_eq!(ws.surface(SurfaceId(101)).unwrap().view.scroll_offset, 7);
        ws.set_view_state(SurfaceId(101), None, Some(None)).unwrap();
        assert!(ws.surface(SurfaceId(101)).unwrap().view.selection.is_none());
    }

    #[test]
    fn splitting_and_closing_panes_keeps_surface_at_the_first_leaf() {
        let mut ws = seeded();
        let tab = ws.sessions[0].tabs[0].id;
        ws.insert_surface(surface(102));
        ws.split_pane(tab, SurfaceId(101), SplitAxis::Row, SurfaceId(102))
            .unwrap();
        ws.insert_surface(surface(103));
        ws.split_pane(tab, SurfaceId(102), SplitAxis::Column, SurfaceId(103))
            .unwrap();

        let entry = ws.tab(tab).unwrap();
        assert_eq!(entry.surface, SurfaceId(101));
        assert_eq!(
            entry.surfaces(),
            vec![SurfaceId(101), SurfaceId(102), SurfaceId(103)]
        );
        assert!(ws.surface_is_attached(SurfaceId(103)));
        assert_eq!(
            ws.split_pane(tab, SurfaceId(999), SplitAxis::Row, SurfaceId(1))
                .unwrap_err()
                .code,
            ErrorCode::NotFound
        );

        // Closing the first Pane: the sibling subtree becomes the root and
        // `surface` moves along.
        let closed = ws.close_pane(tab, SurfaceId(101)).unwrap();
        assert_eq!(closed.surface, SurfaceId(101));
        assert!(closed.tab_closed.is_none());
        assert!(!ws.surfaces.contains_key(&SurfaceId(101)));
        let entry = ws.tab(tab).unwrap();
        assert_eq!(entry.surface, SurfaceId(102));
        assert_eq!(entry.surfaces(), vec![SurfaceId(102), SurfaceId(103)]);

        ws.close_pane(tab, SurfaceId(103)).unwrap();
        assert!(ws.tab(tab).unwrap().layout.is_leaf());

        // The last Pane closes the Tab, with the Q21 cascade.
        let closed = ws.close_pane(tab, SurfaceId(102)).unwrap();
        let tab_closed = closed.tab_closed.unwrap();
        assert_eq!(tab_closed.surfaces, vec![SurfaceId(102)]);
        assert!(tab_closed.needs_reseed);
        assert!(ws.sessions.is_empty());
        assert!(ws.surfaces.is_empty());
    }

    #[test]
    fn closing_a_split_tab_reports_every_pane() {
        let mut ws = seeded();
        let tab = ws.sessions[0].tabs[0].id;
        ws.insert_surface(surface(102));
        ws.split_pane(tab, SurfaceId(101), SplitAxis::Column, SurfaceId(102))
            .unwrap();
        let closed = ws.close_tab(tab).unwrap();
        assert_eq!(closed.surfaces, vec![SurfaceId(101), SurfaceId(102)]);
        assert!(ws.surfaces.is_empty());
    }

    #[test]
    fn split_ratios_are_clamped_and_paths_validated() {
        let mut ws = seeded();
        let tab = ws.sessions[0].tabs[0].id;
        assert_eq!(
            ws.set_split_ratio(tab, &[], SplitRatio::HALF)
                .unwrap_err()
                .code,
            ErrorCode::NotFound,
            "a single Pane has no split"
        );
        ws.insert_surface(surface(102));
        ws.split_pane(tab, SurfaceId(101), SplitAxis::Row, SurfaceId(102))
            .unwrap();
        ws.set_split_ratio(tab, &[], SplitRatio::from_f32(0.01))
            .unwrap();
        let Layout::Split { ratio, .. } = &ws.tab(tab).unwrap().layout else {
            panic!("expected a split");
        };
        assert_eq!(*ratio, SplitRatio::MIN);
        assert!(ws.set_split_ratio(tab, &[0], SplitRatio::HALF).is_err());
        assert!(ws
            .set_split_ratio(TabId(77), &[], SplitRatio::HALF)
            .is_err());
    }

    #[test]
    fn snapshot_matches_the_model() {
        let ws = seeded();
        let snap = ws.snapshot();
        assert_eq!(snap.workspace.sessions.len(), 1);
        assert_eq!(snap.surfaces.len(), 1);
        assert_eq!(snap.surfaces[0].id, SurfaceId(101));
        assert_eq!(snap.surfaces[0].state, SurfaceState::Running);
    }
}

//! Project order: the user arranges projects by dragging them in the
//! sidebar. The order applies to every snapshot (sidebar, Projects summary,
//! bulk menu) and is saved between runs. Projects not placed yet follow the
//! placed ones, in their usual order (the local project first, then by name).

use std::cmp::Ordering;

use super::App;
use crate::ui::sidebar::NodeKey;

impl App {
    /// Sort the snapshot's projects by the user's order.
    pub(super) fn apply_project_order(&mut self) {
        let order = &self.project_order;
        let rank = |name: &str| order.iter().position(|p| p == name).unwrap_or(usize::MAX);
        self.snap.projects.sort_by_key(|p| rank(&p.name));
    }

    /// Put project `name` where `onto` is, shifting the others, if `onto`
    /// lies in the pointer's `direction` (Greater: down). False if nothing
    /// moved.
    ///
    /// Following the direction keeps groups of different heights from
    /// swapping back and forth: once a short group moves below a tall one, the
    /// tall one slides up under the pointer, but it is behind the drag now.
    pub(super) fn move_project(&mut self, name: &str, onto: &str, direction: Ordering) -> bool {
        let projects = &mut self.snap.projects;
        let (Some(from), Some(to)) =
            (projects.iter().position(|p| p.name == name), projects.iter().position(|p| p.name == onto))
        else {
            return false;
        };
        if to.cmp(&from) != direction || direction == Ordering::Equal {
            return false;
        }
        let project = projects.remove(from);
        projects.insert(to, project);
        // The new order of the projects shown, then the remembered ones that
        // aren't running now, so they keep a place for when they come back.
        let shown: Vec<String> = projects.iter().map(|p| p.name.clone()).collect();
        let absent = self.project_order.iter().filter(|p| !shown.contains(p)).cloned();
        self.project_order = shown.iter().cloned().chain(absent).collect();
        self.rebuild_rows();
        true
    }

    /// The project a sidebar row at screen `row` belongs to: a project row,
    /// or one of its services or replicas.
    pub(super) fn project_at_row(&self, row: u16) -> Option<String> {
        let (_, key) = self.hits.sidebar.rows.iter().find(|(r, _)| r.y == row)?;
        let i = self.rows().iter().position(|r| &r.key == key)?;
        match &self.rows()[..=i].iter().rev().find(|r| r.depth <= 1)?.key {
            NodeKey::Project(p) => Some(p.clone()),
            _ => None,
        }
    }
}

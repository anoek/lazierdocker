//! lazydocker's per-item actions, menus and confirmations
//! (docs/reference/lazydocker-inventory.md §3–4).

use super::actions::Action;
use super::overlay::{Menu, MenuItem};
use super::App;
use crate::docker::model::ContainerState;
use crate::docker::ops::DockerOp;
use crate::docker::present;
use crate::ui::sidebar::{NodeKey, Section};

impl App {
    /// `docker compose [-f …] [--project-directory …] -p <project>` for a project,
    /// or an error when we can't locate its compose files.
    fn compose_base(&self, project: &str) -> Result<Vec<String>, String> {
        let mut argv = vec!["docker".to_string(), "compose".to_string()];
        let local = self.snap.project(project).is_some_and(|p| p.local);
        if local {
            for f in &self.compose_files {
                argv.push("-f".into());
                argv.push(f.clone());
            }
        } else {
            let label = |name: &str| {
                self.snap
                    .containers
                    .iter()
                    .find(|c| c.project.as_deref() == Some(project))
                    .and_then(|c| c.labels.get(name).cloned())
            };
            let Some(files) = label("com.docker.compose.project.config_files") else {
                return Err(format!("Compose project '{project}' has no compose files on this machine; run lazierdocker from its directory."));
            };
            for f in files.split(',').filter(|f| !f.is_empty()) {
                argv.push("-f".into());
                argv.push(f.to_string());
            }
            if let Some(dir) = label("com.docker.compose.project.working_dir") {
                argv.push("--project-directory".into());
                argv.push(dir);
            }
        }
        argv.push("-p".into());
        argv.push(project.to_string());
        Ok(argv)
    }

    pub(super) fn compose(&self, project: &str, args: &[&str]) -> Result<Vec<String>, String> {
        let mut argv = self.compose_base(project)?;
        argv.extend(args.iter().map(|s| s.to_string()));
        Ok(argv)
    }

    /// Background compose command, or an error message action.
    fn compose_action(&self, project: &str, args: &[&str]) -> Action {
        match self.compose(project, args) {
            Ok(argv) => Action::Docker(DockerOp::Cli(argv)),
            Err(e) => Action::Error(e),
        }
    }

    /// Attached compose command shown in a terminal pane.
    fn compose_pane(&self, project: &str, args: &[&str]) -> Action {
        match self.compose(project, args) {
            Ok(argv) => Action::RunInPane { title: format!("{project}: {}", args.join(" ")), argv },
            Err(e) => Action::Error(e),
        }
    }

    /// A container's name, or its short id once it is gone.
    pub(super) fn container_name(&self, id: &str) -> String {
        self.snap.container(id).map(|c| c.name.clone()).unwrap_or_else(|| id.chars().take(12).collect())
    }

    /// The docker action for a key on an item; None if the key doesn't apply.
    pub(super) fn docker_key_action(&self, c: char, key: &NodeKey) -> Option<Action> {
        match key {
            NodeKey::Container(id) => self.container_action(c, id),
            NodeKey::Service(p, s) => self.service_action(c, p, s),
            NodeKey::Project(p) => match c {
                'U' => Some(Action::ask(
                    "Up project",
                    format!("Run `docker compose up -d` for project {p}?"),
                    false,
                    self.compose_action(p, &["up", "-d"]),
                )),
                'D' => Some(Action::Menu(self.down_menu(p))),
                'b' => Some(Action::Menu(self.bulk_services_menu(p))),
                _ => None,
            },
            NodeKey::Image(id) => match c {
                'd' => Some(Action::Menu(self.image_remove_menu(id))),
                'b' => Some(Action::Menu(self.bulk_menu(Section::Images))),
                _ => None,
            },
            NodeKey::Volume(name) => match c {
                'd' => Some(Action::Menu(Menu::new(
                    format!("Remove volume {name}"),
                    vec![
                        MenuItem::new(
                            "remove",
                            Action::Docker(DockerOp::RemoveVolume { name: name.clone(), force: false }),
                        )
                        .detail(format!("docker volume rm {name}"))
                        .danger(),
                        MenuItem::new(
                            "force remove",
                            Action::Docker(DockerOp::RemoveVolume { name: name.clone(), force: true }),
                        )
                        .detail(format!("docker volume rm --force {name}"))
                        .danger(),
                    ],
                ))),
                'b' => Some(Action::Menu(self.bulk_menu(Section::Volumes))),
                _ => None,
            },
            NodeKey::Network(id) => match c {
                'd' => {
                    let name = self.snap.network(id).map(|n| n.name.clone()).unwrap_or_default();
                    Some(Action::Menu(Menu::new(
                        format!("Remove network {name}"),
                        vec![MenuItem::new("remove", Action::Docker(DockerOp::RemoveNetwork { id: id.clone() }))
                            .detail(format!("docker network rm {name}"))
                            .danger()],
                    )))
                }
                'b' => Some(Action::Menu(self.bulk_menu(Section::Networks))),
                _ => None,
            },
            NodeKey::Section(s) => match c {
                'b' => Some(Action::Menu(self.bulk_menu(*s))),
                _ => None,
            },
        }
    }

    fn container_action(&self, c: char, id: &str) -> Option<Action> {
        let cont = self.snap.container(id)?;
        let name = cont.name.clone();
        let id = id.to_string();
        Some(match c {
            's' => Action::ask(
                "Stop container",
                format!("Are you sure you want to stop {name}?"),
                false,
                Action::Docker(DockerOp::Stop(id)),
            ),
            'r' => Action::Docker(DockerOp::Restart(id)),
            'S' => Action::Docker(DockerOp::Start(id)),
            'p' => {
                if cont.state == ContainerState::Paused {
                    Action::Docker(DockerOp::Unpause(id))
                } else {
                    Action::Docker(DockerOp::Pause(id))
                }
            }
            'd' => Action::Menu(self.container_remove_menu(&id)),
            'w' => match present::browser_url(&cont.ports) {
                Some(url) => Action::OpenUrl(url),
                None => Action::Flash(format!("{name} has no published ports")),
            },
            'b' => Action::Menu(self.bulk_menu(Section::Containers)),
            _ => return None,
        })
    }

    /// Service keys: compose commands, or plain docker calls on its container
    /// when the project's compose files aren't available here.
    fn service_action(&self, c: char, p: &str, s: &str) -> Option<Action> {
        let container = self.snap.service(p, s)?.container_id.clone();
        let has_files = self.compose_base(p).is_ok();
        Some(match c {
            'u' => self.compose_action(p, &["up", "-d", s]),
            's' => {
                let action = match (&container, has_files) {
                    (Some(id), false) => Action::Docker(DockerOp::Stop(id.clone())),
                    _ => self.compose_action(p, &["stop", s]),
                };
                Action::ask("Stop service", format!("Are you sure you want to stop service {s}?"), false, action)
            }
            'r' => match (&container, has_files) {
                (Some(id), false) => Action::Docker(DockerOp::Restart(id.clone())),
                _ => self.compose_action(p, &["restart", s]),
            },
            'S' => match (&container, has_files) {
                (Some(id), false) => Action::Docker(DockerOp::Start(id.clone())),
                _ => self.compose_action(p, &["start", s]),
            },
            'p' => match container.as_deref().and_then(|id| self.snap.container(id)) {
                Some(c) if c.state == ContainerState::Paused => Action::Docker(DockerOp::Unpause(c.id.clone())),
                Some(c) => Action::Docker(DockerOp::Pause(c.id.clone())),
                None => return None,
            },
            'd' => Action::Menu(Menu::new(
                format!("Remove service {s}"),
                vec![
                    MenuItem::new("remove containers", self.compose_action(p, &["rm", "--stop", "--force", s]))
                        .detail(format!("compose rm --stop --force {s}"))
                        .danger(),
                    MenuItem::new(
                        "remove containers and volumes",
                        self.compose_action(p, &["rm", "--stop", "--force", "-v", s]),
                    )
                    .detail(format!("compose rm --stop --force -v {s}"))
                    .danger(),
                ],
            )),
            'R' => Action::Menu(Menu::new(
                format!("Restart service {s}"),
                vec![
                    MenuItem::new("restart", self.compose_action(p, &["restart", s]))
                        .detail(format!("compose restart {s}")),
                    MenuItem::new("recreate", self.compose_action(p, &["up", "-d", "--force-recreate", s]))
                        .detail(format!("compose up -d --force-recreate {s}")),
                    MenuItem::new("rebuild", self.compose_pane(p, &["up", "-d", "--build", s]))
                        .detail(format!("compose up -d --build {s}")),
                ],
            )),
            'U' => Action::ask(
                "Up project",
                format!("Run `docker compose up -d` for project {p}?"),
                false,
                self.compose_action(p, &["up", "-d"]),
            ),
            'D' => Action::Menu(self.down_menu(p)),
            'w' => match container
                .as_deref()
                .and_then(|id| self.snap.container(id))
                .and_then(|c| present::browser_url(&c.ports))
            {
                Some(url) => Action::OpenUrl(url),
                None => Action::Flash(format!("{s} has no published ports")),
            },
            'b' => Action::Menu(self.bulk_services_menu(p)),
            _ => return None,
        })
    }

    fn container_remove_menu(&self, id: &str) -> Menu {
        let name = self.container_name(id);
        Menu::new(
            format!("Remove {name}"),
            vec![
                MenuItem::new(
                    "remove",
                    Action::Docker(DockerOp::RemoveContainer { id: id.into(), force: false, volumes: false }),
                )
                .detail(format!("docker rm {name}"))
                .danger(),
                MenuItem::new(
                    "remove with volumes",
                    Action::Docker(DockerOp::RemoveContainer { id: id.into(), force: false, volumes: true }),
                )
                .detail(format!("docker rm --volumes {name}"))
                .danger(),
            ],
        )
    }

    fn image_remove_menu(&self, id: &str) -> Menu {
        let name = self.snap.image(id).map(|i| i.display()).unwrap_or_default();
        let op = |force, no_prune| Action::Docker(DockerOp::RemoveImage { id: id.into(), force, no_prune });
        Menu::new(
            format!("Remove image {name}"),
            vec![
                MenuItem::new("remove", op(false, false)).detail(format!("docker image rm {name}")).danger(),
                MenuItem::new("remove without deleting untagged parents", op(false, true))
                    .detail("--no-prune")
                    .danger(),
                MenuItem::new("force remove", op(true, false)).detail("--force").danger(),
                MenuItem::new("force remove without deleting untagged parents", op(true, true))
                    .detail("--no-prune --force")
                    .danger(),
            ],
        )
    }

    fn down_menu(&self, p: &str) -> Menu {
        Menu::new(
            format!("Down project {p}"),
            vec![
                MenuItem::new("down", self.compose_action(p, &["down"])).detail("compose down").danger(),
                MenuItem::new("down with volumes", self.compose_action(p, &["down", "--volumes"]))
                    .detail("compose down --volumes")
                    .danger(),
            ],
        )
    }

    pub(super) fn bulk_services_menu(&self, p: &str) -> Menu {
        Menu::new(
            format!("Bulk commands: {p}"),
            vec![
                MenuItem::new("up", self.compose_action(p, &["up", "-d"])).detail("compose up -d"),
                MenuItem::new("up (attached)", self.compose_pane(p, &["up"])).detail("compose up"),
                MenuItem::new("stop", self.compose_action(p, &["stop"])).detail("compose stop"),
                MenuItem::new("pull", self.compose_pane(p, &["pull"])).detail("compose pull"),
                MenuItem::new("build", self.compose_pane(p, &["build", "--parallel", "--force-rm"]))
                    .detail("compose build --parallel --force-rm"),
                MenuItem::new("down", self.compose_action(p, &["down"])).detail("compose down").danger(),
                MenuItem::new("down with volumes", self.compose_action(p, &["down", "--volumes"]))
                    .detail("compose down --volumes")
                    .danger(),
                MenuItem::new("down and remove images", self.compose_action(p, &["down", "--rmi", "all"]))
                    .detail("compose down --rmi all")
                    .danger(),
                MenuItem::new(
                    "down with volumes and remove images",
                    self.compose_action(p, &["down", "--volumes", "--rmi", "all"]),
                )
                .detail("compose down --volumes --rmi all")
                .danger(),
            ],
        )
    }

    fn bulk_menu(&self, section: Section) -> Menu {
        let prune = |what: &str, op: DockerOp| {
            MenuItem::new(
                format!("prune unused {what}"),
                Action::ask(
                    &format!("Prune {what}"),
                    format!("Are you sure you want to prune all unused {what}?"),
                    true,
                    Action::Docker(op),
                ),
            )
            .detail(format!("docker {} prune", what.trim_end_matches('s')))
            .danger()
        };
        match section {
            Section::Containers => {
                let all: Vec<String> = self.snap.containers.iter().map(|c| c.id.clone()).collect();
                let running: Vec<String> =
                    self.snap.containers.iter().filter(|c| c.is_running()).map(|c| c.id.clone()).collect();
                Menu::new(
                    "Bulk commands: containers",
                    vec![
                        MenuItem::new(
                            "stop all containers",
                            Action::ask(
                                "Stop all",
                                format!("Stop all {} running containers?", running.len()),
                                true,
                                Action::Docker(DockerOp::StopAll(running)),
                            ),
                        )
                        .danger(),
                        MenuItem::new(
                            "remove all containers (forced)",
                            Action::ask(
                                "Remove all",
                                format!("Force-remove all {} containers?", all.len()),
                                true,
                                Action::Docker(DockerOp::RemoveAll(all)),
                            ),
                        )
                        .danger(),
                        prune("containers", DockerOp::PruneContainers),
                    ],
                )
            }
            Section::Images => Menu::new("Bulk commands: images", vec![prune("images", DockerOp::PruneImages)]),
            Section::Volumes => Menu::new("Bulk commands: volumes", vec![prune("volumes", DockerOp::PruneVolumes)]),
            Section::Networks => Menu::new("Bulk commands: networks", vec![prune("networks", DockerOp::PruneNetworks)]),
            Section::Projects => {
                let items = self
                    .snap
                    .projects
                    .iter()
                    .map(|p| MenuItem::new(format!("{} …", p.name), Action::Menu(self.bulk_services_menu(&p.name))))
                    .collect();
                Menu::new("Bulk commands: projects", items)
            }
        }
    }

    /// Docker items for an item's context menu: every key action that applies,
    /// with its key shown.
    pub(super) fn docker_menu_items(&self, key: &NodeKey) -> Vec<MenuItem> {
        let candidates: &[(char, &str)] = match key {
            NodeKey::Container(_) => &[
                ('s', "Stop"),
                ('r', "Restart"),
                ('S', "Start"),
                ('p', "Pause / unpause"),
                ('d', "Remove…"),
                ('w', "Open in browser"),
                ('b', "Bulk…"),
            ],
            NodeKey::Service(..) => &[
                ('u', "Up"),
                ('s', "Stop"),
                ('r', "Restart"),
                ('S', "Start"),
                ('R', "Restart / recreate / rebuild…"),
                ('p', "Pause / unpause"),
                ('d', "Remove…"),
                ('w', "Open in browser"),
                ('b', "Bulk…"),
            ],
            NodeKey::Project(_) => &[('U', "Up"), ('D', "Down…"), ('b', "Bulk…")],
            NodeKey::Image(_) | NodeKey::Volume(_) | NodeKey::Network(_) => &[('d', "Remove…"), ('b', "Prune…")],
            // The Projects section's bulk menu is only a list of per-project
            // menus, which the projects' own context menus already offer.
            NodeKey::Section(s) if *s != Section::Projects => &[('b', "Bulk…")],
            NodeKey::Section(_) => &[],
        };
        candidates
            .iter()
            .filter_map(|(c, label)| {
                let action = self.docker_key_action(*c, key)?;
                // Leave out items that can only say why they can't run (no
                // published port to open, no compose files on this machine).
                if matches!(action, Action::Flash(_) | Action::Error(_)) {
                    return None;
                }
                let danger = matches!(c, 'd' | 'D');
                let item = MenuItem::new(*label, action).key(&c.to_string());
                Some(if danger { item.danger() } else { item })
            })
            .collect()
    }
}

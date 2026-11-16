//! Runs only when LAZIERDOCKER_TEST_DOCKER_HOST points at a disposable daemon
//! (see docs: docker-in-docker fixture with the `demo` compose project).

use lazierdocker::docker::client::Docker;

fn test_host() -> Option<String> {
    std::env::var("LAZIERDOCKER_TEST_DOCKER_HOST").ok().filter(|s| !s.is_empty())
}

#[tokio::test]
async fn snapshot_sees_demo_project() {
    let Some(host) = test_host() else {
        eprintln!("skipped: LAZIERDOCKER_TEST_DOCKER_HOST not set");
        return;
    };
    let docker = Docker::connect_to(&host).unwrap();
    let snap = docker.snapshot(None).await.unwrap();
    let demo = snap.project("demo").expect("demo project");
    let names: Vec<_> = demo.services.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, vec!["db", "web", "worker"]);
    assert!(snap.containers.iter().any(|c| c.name == "loner"));
    assert!(snap.standalone_containers().any(|c| c.name == "exited"));
    assert!(snap.images.iter().any(|i| i.name == "nginx"));
    assert!(snap.volumes.iter().any(|v| v.name == "demo_dbdata"));
    assert!(snap.networks.iter().any(|n| n.name == "demo_default"));
}

#[tokio::test]
async fn restart_container_via_op() {
    use lazierdocker::docker::ops::{run, DockerOp};
    let Some(host) = test_host() else { return };
    let docker = Docker::connect_to(&host).unwrap();
    let snap = docker.snapshot(None).await.unwrap();
    let loner = snap.containers.iter().find(|c| c.name == "loner").unwrap();
    let before = docker.api.inspect_container(&loner.id, None).await.unwrap().state.unwrap().started_at.unwrap();
    run(&docker, DockerOp::Restart(loner.id.clone())).await.unwrap();
    let after = docker.api.inspect_container(&loner.id, None).await.unwrap().state.unwrap().started_at.unwrap();
    assert_ne!(before, after);
    // Errors come back as Err, not panics.
    assert!(run(&docker, DockerOp::Start("no-such-container".into())).await.is_err());
}

/// Images often run as a UID with no /etc/passwd entry; exec must still give a shell.
#[test]
fn exec_shell_script_works_without_passwd_entry() {
    let Some(host) = test_host() else { return };
    let script = lazierdocker::app::panes::EXEC_SHELL_SCRIPT;
    for user in ["4242", "root"] {
        let mut child = std::process::Command::new("docker")
            .env("DOCKER_HOST", &host)
            .args(["exec", "-i", "--user", user, "loner", "/bin/sh", "-c", script])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        use std::io::Write;
        child.stdin.take().unwrap().write_all(b"echo shell-alive\nexit\n").unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(String::from_utf8_lossy(&out.stdout).contains("shell-alive"), "user {user}: {out:?}");
    }
}

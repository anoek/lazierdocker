//! Runs only when LAZIERDOCKER_TEST_DOCKER_HOST points at a disposable daemon
//! (see docs: docker-in-docker fixture with the `demo` compose project).

use lazierdocker::docker::client::Docker;

fn test_host() -> Option<String> {
    std::env::var("LAZIERDOCKER_TEST_DOCKER_HOST")
        .ok()
        .filter(|s| !s.is_empty())
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

//! `random-bytes`, `credential-status` and `credential-forget` lower to the contract form and survive the formatter.

use orbital_core_base::schema::TraitReference;
use orbital_lolo::{format_program, parse_and_lower_str, parse_str};
use serde_json::{json, Value};

fn program(effect: &str) -> String {
    let config = if effect.contains("@config.token") { r#"config { token: secret = "TOKEN_NAME" }"# } else { "" };
    format!(
        r#"orbital DemoOrbital {{
  entity Demo [runtime] {{
    id : string!
  }}

  trait DemoFlow -> Demo [interaction, instance] {{
    {config}
    initial: idle
    state idle {{
      INIT -> idle
        (render-ui main {{ type: "box" }})
      GO -> idle
        {effect}
    }}
  }}
}}"#
    )
}

fn effects_of(src: &str) -> Vec<Value> {
    let schema = parse_and_lower_str(src).expect("fixture must lower");
    let TraitReference::Inline(td) = &schema.orbitals[0].traits[0] else {
        panic!("expected an inline trait");
    };
    let sm = serde_json::to_value(&td.state_machine).unwrap();
    sm["transitions"]
        .as_array()
        .expect("transitions")
        .iter()
        .find(|t| t["event"] == "GO")
        .and_then(|t| t["effects"].as_array().cloned())
        .expect("transition effects")
}

#[test]
fn random_bytes_lowers_to_the_contract_form() {
    let effects = effects_of(&program("(random-bytes 32 { emit: { success: DRAWN, failure: DRAW_FAILED } })"));
    assert_eq!(
        effects,
        vec![json!(["random-bytes", 32.0, { "emit": { "success": "DRAWN", "failure": "DRAW_FAILED" } }])]
    );
}

#[test]
fn the_credential_effects_lower_with_the_knob_binding() {
    for head in ["credential-status", "credential-forget"] {
        let effects = effects_of(&program(&format!("({head} @config.token {{ emit: {{ success: DONE }} }})")));
        assert_eq!(
            effects,
            vec![json!([head, "@config.token", { "emit": { "success": "DONE" } }])],
            "{head}"
        );
    }
}

#[test]
fn control_options_are_optional() {
    assert_eq!(effects_of(&program("(random-bytes 8)")), vec![json!(["random-bytes", 8.0])]);
}

#[test]
fn the_formatter_keeps_the_three_heads() {
    for call in [
        "(random-bytes 16 { emit: { success: DRAWN } })",
        "(credential-status @config.token { emit: { success: DONE } })",
        "(credential-forget @config.token)",
    ] {
        let src = program(call);
        let formatted = format_program(&parse_str(&src).expect("parse"));
        let head = call.split_whitespace().next().unwrap();
        assert!(formatted.contains(head), "{formatted}");
        assert_eq!(effects_of(&formatted), effects_of(&src), "{call}");
    }
}

const OAUTH_CLIENT: &str = r#"{ tag: google, mode: real, issuer: "https://accounts.google.com", clientId: @config.token, clientSecret: @config.token, clientAuth: post }"#;

fn oauth_client_json() -> Value {
    json!({ "tag": "google", "mode": "real", "issuer": "https://accounts.google.com", "clientId": "@config.token", "clientSecret": "@config.token", "clientAuth": "post" })
}

#[test]
fn the_oauth_effects_lower_with_bare_words_as_strings() {
    let first = |call: &str| effects_of(&program(&format!("{call}\n        (credential-forget @config.token)"))).remove(0);
    let status = first("(oauth-connection-status google { emit: { success: DONE } })");
    assert_eq!(status, json!(["oauth-connection-status", "google", { "emit": { "success": "DONE" } }]));
    let forget = first("(oauth-connection-forget github)");
    assert_eq!(forget, json!(["oauth-connection-forget", "github"]));
    let authorize = first(&format!(
        r#"(oauth-authorize {OAUTH_CLIENT} {{ redirectUri: "https://a/cb", scope: ["openid"], params: {{ access_type: "offline" }} }} {{ emit: {{ success: DONE }} }})"#
    ));
    assert_eq!(
        authorize,
        json!(["oauth-authorize", oauth_client_json(),
            { "redirectUri": "https://a/cb", "scope": ["openid"], "params": { "access_type": "offline" } },
            { "emit": { "success": "DONE" } }])
    );
}

#[test]
fn the_oauth_client_http_spelling_lowers_to_the_oauth_endpoint_target() {
    let call = format!(
        "(http POST oauth-endpoint token {{ auth: {{ scheme: oauth-client, op: connect, client: {OAUTH_CLIENT}, provider: google, code: @payload.code, state: @payload.state }} }} {{ emit: {{ success: DONE, failure: FAILED }} }})"
    );
    let effects = effects_of(&program(&call));
    assert_eq!(
        effects,
        vec![json!(["http", "POST", "oauth-endpoint", "token",
            { "auth": { "scheme": "oauth-client", "op": "connect", "client": oauth_client_json(), "provider": "google",
                "code": "@payload.code", "state": "@payload.state" } },
            { "emit": { "success": "DONE", "failure": "FAILED" } }])]
    );
    let discovery = effects_of(&program(&format!(
        "(http GET \"https://accounts.google.com\" \"/.well-known/openid-configuration\" {{ auth: {{ scheme: oauth-client, op: discovery, client: {OAUTH_CLIENT} }} }})"
    )));
    assert_eq!(discovery[0][1], "GET");
}

#[test]
fn the_formatter_keeps_the_oauth_heads() {
    for call in [
        "(oauth-connection-status google { emit: { success: DONE } })".to_string(),
        "(oauth-connection-forget google)".to_string(),
        format!("(oauth-authorize {OAUTH_CLIENT} {{ redirectUri: \"https://a/cb\" }})"),
    ] {
        let src = program(&format!("{call}\n        (credential-forget @config.token)"));
        let formatted = format_program(&parse_str(&src).expect("parse"));
        assert!(formatted.contains(call.split_whitespace().next().unwrap()), "{formatted}");
        assert_eq!(effects_of(&formatted), effects_of(&src), "{call}");
    }
}

#[test]
fn the_clients_map_and_the_authorize_key_lower_as_written() {
    let first = |call: &str| effects_of(&program(&format!("{call}\n        (credential-forget @config.token)"))).remove(0);
    let github = OAUTH_CLIENT.replace("tag: google", "tag: github");
    let routed = first(&format!(
        "(http POST oauth-endpoint token {{ auth: {{ scheme: oauth-client, op: code, clients: {{ google: {OAUTH_CLIENT}, github: {github} }}, code: @payload.code, state: @payload.state }} }} {{ emit: {{ success: DONE }} }})"
    ));
    let mut github_json = oauth_client_json();
    github_json["tag"] = json!("github");
    assert_eq!(
        routed[4]["auth"]["clients"],
        json!({ "google": oauth_client_json(), "github": github_json })
    );
    let keyed = first(&format!(
        r#"(oauth-authorize {OAUTH_CLIENT} {{ redirectUri: "https://a/cb" }} {{ key: @entity.id, emit: {{ success: DONE, failure: FAILED }} }})"#
    ));
    assert_eq!(keyed[3], json!({ "key": "@entity.id", "emit": { "success": "DONE", "failure": "FAILED" } }));
}


#[cfg(unix)]
mod supervisor {
    use orbital_core_base::effect_types::{Effect, RuntimeEnvironment};
    use orbital_core_base::schema::{OrbitalSchema, SExpression, TraitReference};
    use orbital_core_base::value::Value;
    use orbital_core_runtime::config::KernelConfig;
    use orbital_core_runtime::entity_source::{EntitySource, InMemoryEntityCache};
    use orbital_core_runtime::kernel::RuntimeKernel;
    use orbital_core_runtime::os_host::completion::{CompletionAnswerer, CompletionDone, KernelCompletionSink};
    use orbital_core_runtime::os_host::{answer_os, OsHandles, OsHost};
    use orbital_core_runtime::resumable::{BlockingHost, CallSite, DispatchError, DispatchOutcomes, IoRequest};
    use orbital_lolo::parse_and_lower;
    use serde_json::{json, Value as Json};
    use std::path::Path;
    use std::process::{Command, Stdio};
    use std::sync::{Arc, Mutex, MutexGuard};
    use std::time::{Duration, Instant};

    type Events = Vec<(String, Value)>;

    fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
        m.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn emits(outcomes: &DispatchOutcomes) -> Events {
        outcomes
            .iter()
            .flat_map(|(_, result)| result.effect_results.iter())
            .filter_map(|r| match &r.effect {
                Effect::Emit { event, payload } => Some((event.clone(), payload.clone())),
                _ => None,
            })
            .collect()
    }

    fn text(payload: &Value, field: &str) -> Option<String> {
        match payload.get_path(field) {
            Some(Value::String(s)) => Some(s.clone()),
            _ => None,
        }
    }

    fn number(payload: &Value, field: &str) -> Option<f64> {
        match payload.get_path(field) {
            Some(Value::Number(n)) => Some(*n),
            _ => None,
        }
    }

    fn count(events: &[(String, Value)], name: &str) -> usize {
        events.iter().filter(|(event, _)| event == name).count()
    }

    fn started(events: &[(String, Value)]) -> Vec<(String, i64)> {
        events
            .iter()
            .filter(|(event, _)| event == "CHILD_STARTED")
            .map(|(_, p)| (text(p, "name").expect("CHILD_STARTED.name"), number(p, "pid").expect("CHILD_STARTED.pid") as i64))
            .collect()
    }

    fn pids_of(events: &[(String, Value)], name: &str) -> Vec<i64> {
        started(events).into_iter().filter(|(n, _)| n == name).map(|(_, pid)| pid).collect()
    }

    fn kill_signal(signal: &str, pid: i64) -> bool {
        Command::new("/bin/kill")
            .args([signal, &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    fn alive(pid: i64) -> bool {
        kill_signal("-0", pid)
    }

    fn child(name: &str, restart: &str) -> Json {
        json!({ "name": name, "command": "/bin/sleep", "args": ["30"], "restart": restart })
    }

    fn lowered(strategy: &str, children: Json, max_restarts: u32) -> OrbitalSchema {
        let atom = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../packages/almadar-std/behaviors/lolo/flow/atoms/std-supervisor.lolo");
        let mut schema = parse_and_lower(&atom).expect("std-supervisor lowers");
        let mut grants = schema.os_grants.clone().unwrap_or_default();
        grants.exec.push("/bin/sleep".to_string());
        grants.exec.push("/bin/sh".to_string());
        schema.os_grants = Some(grants);
        let TraitReference::Inline(td) = &mut schema.orbitals[0].traits[0] else {
            panic!("the supervisor trait is inline");
        };
        let knobs = td.config.as_mut().expect("the trait declares config knobs");
        for (knob, value) in [
            ("strategy", json!(strategy)),
            ("children", children),
            ("maxRestarts", json!(max_restarts)),
            ("periodMs", json!(60000)),
        ] {
            knobs.get_mut(knob).unwrap_or_else(|| panic!("config knob {knob}")).default =
                Some(serde_json::from_value::<SExpression>(value).expect("knob value"));
        }
        schema
    }

    struct Rig {
        kernel: Arc<RuntimeKernel>,
        sink: Arc<KernelCompletionSink>,
        handles: Arc<OsHandles>,
        events: Arc<Mutex<Events>>,
        errors: Arc<Mutex<Vec<String>>>,
    }

    impl Rig {
        fn start(schema: OrbitalSchema) -> Rig {
            let grants = Arc::new(schema.program_os_grants());
            assert!(!grants.stdio.is_empty(), "the atom's stdio grant must reach the program grants");
            let store: Arc<dyn EntitySource> = Arc::new(InMemoryEntityCache::new());
            let kernel = Arc::new(RuntimeKernel::new(RuntimeEnvironment::Server, store.clone(), KernelConfig::default()));
            kernel.register_schema(schema).expect("register");
            let handles = Arc::new(OsHandles::default());
            let answer: Arc<CompletionAnswerer> = {
                let (handles, grants) = (handles.clone(), grants.clone());
                Arc::new(move |request: &IoRequest| match request {
                    IoRequest::Os { .. } => {
                        let on_event = |_: Option<&CallSite>, _: Value| {};
                        answer_os(Some(&OsHost { grants: &grants, handles: &handles, on_event: &on_event }), request)
                    }
                    other => BlockingHost { source: store.as_ref(), service: None, substrate: None }.answer(other),
                })
            };
            let events: Arc<Mutex<Events>> = Arc::default();
            let errors: Arc<Mutex<Vec<String>>> = Arc::default();
            let on_done: Arc<CompletionDone> = {
                let (events, errors) = (events.clone(), errors.clone());
                Arc::new(move |_site: &CallSite, event: &str, result: Result<DispatchOutcomes, DispatchError>| match result {
                    Ok(outcomes) => lock(&events).extend(emits(&outcomes)),
                    Err(error) => lock(&errors).push(format!("{event}: {error}")),
                })
            };
            let sink = Arc::new(KernelCompletionSink::new(kernel.clone(), answer, on_done));
            handles.completions.set_sink(sink.clone());
            let rig = Rig { kernel, sink, handles, events, errors };
            let progress = rig.kernel.start_dispatch("Supervisor", None, "START", Value::Null, None, None).expect("START");
            let outcomes = rig.sink.drive(progress).expect("START drives to its end");
            lock(&rig.events).extend(emits(&outcomes));
            rig
        }

        fn events(&self) -> Events {
            lock(&self.events).clone()
        }

        fn state(&self) -> String {
            self.kernel.snapshot_trait("Supervisor", None).state
        }

        fn wait_until(&self, what: &str, done: impl Fn(&[(String, Value)]) -> bool) {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if done(&lock(&self.events)) {
                    return;
                }
                if Instant::now() >= deadline {
                    let names: Vec<String> = self.events().iter().map(|(e, p)| format!("{e} {p:?}")).collect();
                    panic!("timed out after 5s waiting for {what}; events {names:?}; dispatch errors {:?}", lock(&self.errors));
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }

        fn wait_dead(&self, pids: &[i64]) {
            let deadline = Instant::now() + Duration::from_secs(5);
            while pids.iter().any(|pid| alive(*pid)) {
                if Instant::now() >= deadline {
                    let left: Vec<String> = pids.iter().filter(|p| alive(**p)).map(|p| p.to_string()).collect();
                    let listing = std::process::Command::new("ps").args(["-o", "pid,ppid,stat,command", "-p", &left.join(",")]).output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
                    panic!("children still alive after 5s: {left:?}\n{listing}\nall pids {pids:?}; events {:?}", self.events());
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }

    impl Drop for Rig {
        fn drop(&mut self) {
            for (_, pid) in started(&self.events()) {
                kill_signal("-9", pid);
            }
            self.handles.close_all();
        }
    }

    #[test]
    fn a_killed_child_restarts_alone_under_one_for_one_until_the_intensity_is_spent() {
        let rig = Rig::start(lowered("one-for-one", json!([child("a", "permanent"), child("b", "permanent")]), 2));
        let booted = rig.events();
        let names: Vec<String> = started(&booted).into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, vec!["a", "b"], "CHILD_STARTED names in start order: {booted:?}");
        assert_eq!(count(&booted, "SUPERVISOR_READY"), 1, "{booted:?}");
        assert_eq!(rig.state(), "running");
        let (a0, b0) = (pids_of(&booted, "a")[0], pids_of(&booted, "b")[0]);
        assert_ne!(a0, b0);
        assert!(alive(a0) && alive(b0));

        let mut a_pids = vec![a0];
        for round in 1..=2usize {
            assert!(kill_signal("-9", *a_pids.last().unwrap()), "SIGKILL delivered");
            rig.wait_until(&format!("restart {round} of a"), |ev| {
                count(ev, "CHILD_RESTARTING") >= round && pids_of(ev, "a").len() > round
            });
            let events = rig.events();
            let restarting: Vec<&Value> = events.iter().filter(|(e, _)| e == "CHILD_RESTARTING").map(|(_, p)| p).collect();
            assert_eq!(text(restarting[round - 1], "name").as_deref(), Some("a"));
            assert_eq!(text(restarting[round - 1], "strategy").as_deref(), Some("one-for-one"));
            assert_eq!(number(restarting[round - 1], "restarts"), Some(round as f64));
            let fresh = *pids_of(&events, "a").last().unwrap();
            assert!(!a_pids.contains(&fresh), "round {round}: the restarted a reuses a pid {fresh} of {a_pids:?}");
            assert!(alive(fresh));
            a_pids.push(fresh);
            assert_eq!(pids_of(&events, "b"), vec![b0], "round {round}: one-for-one must not respawn sibling b");
            assert!(alive(b0), "round {round}: sibling b must survive");
        }

        assert!(kill_signal("-9", *a_pids.last().unwrap()));
        rig.wait_until("SUPERVISOR_GAVE_UP", |ev| count(ev, "SUPERVISOR_GAVE_UP") == 1);
        let events = rig.events();
        let gave_up_at = events.iter().position(|(e, _)| e == "SUPERVISOR_GAVE_UP").unwrap();
        let gave_up = &events[gave_up_at].1;
        assert_eq!(text(gave_up, "reason").as_deref(), Some("intensity"));
        assert_eq!(text(gave_up, "child").as_deref(), Some("a"));
        assert_eq!(number(gave_up, "restarts"), Some(2.0));
        let spawned_before = started(&events).len();

        std::thread::sleep(Duration::from_millis(500));
        let after = rig.events();
        assert_eq!(started(&after).len(), spawned_before, "no child may start after the give-up: {after:?}");
        assert!(after[gave_up_at + 1..].iter().all(|(e, _)| e != "CHILD_STARTED" && e != "CHILD_RESTARTING"), "{after:?}");
        assert_eq!(count(&after, "CHILD_RESTARTING"), 2);
        assert_eq!(rig.state(), "idle");
        let every: Vec<i64> = started(&after).into_iter().map(|(_, pid)| pid).collect();
        rig.wait_dead(&every);
    }

    #[test]
    fn a_killed_temporary_child_is_retired_and_never_respawned() {
        let rig = Rig::start(lowered("one-for-one", json!([child("t", "temporary"), child("p", "permanent")]), 2));
        let booted = rig.events();
        let (t0, p0) = (pids_of(&booted, "t")[0], pids_of(&booted, "p")[0]);
        assert!(kill_signal("-9", t0));
        rig.wait_until("CHILD_RETIRED for t", |ev| count(ev, "CHILD_RETIRED") == 1);
        let events = rig.events();
        let retired = &events.iter().find(|(e, _)| e == "CHILD_RETIRED").unwrap().1;
        assert_eq!(text(retired, "name").as_deref(), Some("t"));
        assert_eq!(retired.get_path("abnormal"), Some(&Value::Bool(true)));
        assert_eq!(number(retired, "code"), Some(-1.0));

        std::thread::sleep(Duration::from_millis(500));
        let after = rig.events();
        assert_eq!(count(&after, "CHILD_RESTARTING"), 0, "{after:?}");
        assert_eq!(pids_of(&after, "t"), vec![t0], "a temporary child is never respawned");
        assert_eq!(started(&after).len(), 2, "{after:?}");
        assert!(alive(p0), "control: the permanent sibling is untouched");
        assert_eq!(rig.state(), "running");
    }

    fn started_after_restart(events: &[(String, Value)]) -> Vec<(String, i64)> {
        let at = events.iter().position(|(e, _)| e == "CHILD_RESTARTING").expect("a CHILD_RESTARTING event");
        started(&events[at..])
    }

    #[test]
    fn one_for_all_restarts_every_child_in_start_order() {
        let rig = Rig::start(lowered("one-for-all", json!([child("a", "permanent"), child("b", "permanent"), child("c", "permanent")]), 3));
        let booted = rig.events();
        let (a0, b0, c0) = (pids_of(&booted, "a")[0], pids_of(&booted, "b")[0], pids_of(&booted, "c")[0]);
        assert!(alive(a0) && alive(b0) && alive(c0));
        assert!(kill_signal("-9", b0));
        rig.wait_until("all three restarted", |ev| count(ev, "CHILD_RESTARTING") == 1 && started_after_restart(ev).len() == 3);
        let events = rig.events();
        let restarting = &events.iter().find(|(e, _)| e == "CHILD_RESTARTING").unwrap().1;
        assert_eq!(text(restarting, "name").as_deref(), Some("b"));
        assert_eq!(text(restarting, "strategy").as_deref(), Some("one-for-all"));
        let fresh = started_after_restart(&events);
        let order: Vec<&str> = fresh.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(order, vec!["a", "b", "c"], "respawn order: {events:?}");
        let old = [a0, b0, c0];
        for (name, pid) in &fresh {
            assert!(!old.contains(pid), "{name} reuses an old pid {pid}");
            assert!(alive(*pid), "{name} must be running");
        }
        rig.wait_dead(&[a0, c0]);
        let every: Vec<i64> = started(&events).into_iter().map(|(_, pid)| pid).collect();
        assert_eq!(every.iter().filter(|pid| alive(**pid)).count(), 3, "control: exactly the new three are alive; pids {every:?}");
        assert_eq!(rig.state(), "running");
    }

    #[test]
    fn rest_for_one_restarts_only_the_failed_child_and_those_after_it() {
        let rig = Rig::start(lowered("rest-for-one", json!([child("a", "permanent"), child("b", "permanent"), child("c", "permanent")]), 3));
        let booted = rig.events();
        let (a0, b0, c0) = (pids_of(&booted, "a")[0], pids_of(&booted, "b")[0], pids_of(&booted, "c")[0]);
        assert!(kill_signal("-9", b0));
        rig.wait_until("b and c restarted", |ev| count(ev, "CHILD_RESTARTING") == 1 && started_after_restart(ev).len() == 2);
        let events = rig.events();
        let fresh = started_after_restart(&events);
        let order: Vec<&str> = fresh.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(order, vec!["b", "c"], "respawn order: {events:?}");
        assert_eq!(pids_of(&events, "a"), vec![a0], "control: a is never respawned");
        assert!(alive(a0), "control: a is untouched");
        let (b1, c1) = (fresh[0].1, fresh[1].1);
        assert!(b1 != b0 && c1 != c0 && alive(b1) && alive(c1));
        rig.wait_dead(&[c0]);
        assert_eq!(rig.state(), "running");
    }

    #[test]
    fn transient_restarts_on_abnormal_exit_and_retires_on_normal_exit() {
        let quitter = json!({ "name": "n", "command": "/bin/sh", "args": ["-c", "exit 0"], "restart": "transient" });
        let rig = Rig::start(lowered("one-for-one", json!([child("k", "transient"), quitter]), 3));
        let booted = rig.events();
        let k0 = pids_of(&booted, "k")[0];
        rig.wait_until("CHILD_RETIRED for n", |ev| count(ev, "CHILD_RETIRED") == 1);
        let events = rig.events();
        let retired = &events.iter().find(|(e, _)| e == "CHILD_RETIRED").unwrap().1;
        assert_eq!(text(retired, "name").as_deref(), Some("n"));
        assert_eq!(retired.get_path("abnormal"), Some(&Value::Bool(false)));
        assert_eq!(number(retired, "code"), Some(0.0), "a clean exit reports its code 0, not the signalled -1");
        assert_eq!(count(&events, "CHILD_RESTARTING"), 0, "a normal exit must not restart: {events:?}");

        assert!(kill_signal("-9", k0));
        rig.wait_until("k restarted", |ev| count(ev, "CHILD_RESTARTING") == 1 && pids_of(ev, "k").len() == 2);
        std::thread::sleep(Duration::from_millis(300));
        let after = rig.events();
        let restarting = &after.iter().find(|(e, _)| e == "CHILD_RESTARTING").unwrap().1;
        assert_eq!(text(restarting, "name").as_deref(), Some("k"));
        assert!(alive(*pids_of(&after, "k").last().unwrap()), "control: the killed transient child is running again");
        assert_eq!(pids_of(&after, "n").len(), 1, "the normally exited child is never respawned: {after:?}");
        assert_eq!(count(&after, "CHILD_RETIRED"), 1);
        assert_eq!(rig.state(), "running");
    }
}

fn plain_program(effect: &str) -> String {
    program(effect).replace("    config { token: secret = \"TOKEN_NAME\" }\n", "")
}

#[test]
fn unicode_escapes_lower_to_the_characters_and_the_formatter_prints_control_characters_canonically() {
    let src = plain_program(r#"(set @entity.id "a\u{1F}b\u{0}c\u{1F600}d\u{e9}\u{7f}")"#);
    let effects = effects_of(&src);
    assert_eq!(effects, vec![json!(["set", "@entity.id", "a\u{1F}b\u{0}c\u{1F600}d\u{E9}\u{7F}"])]);
    let formatted = format_program(&parse_str(&src).expect("parse"));
    assert!(formatted.contains(r#""a\u{1F}b\u{0}c😀dé\u{7F}""#), "{formatted}");
    assert_eq!(effects_of(&formatted), effects);
    assert_eq!(format_program(&parse_str(&formatted).expect("reparse")), formatted);
    let control = plain_program(r#"(set @entity.id "plain \n \t text")"#);
    let again = format_program(&parse_str(&control).expect("parse"));
    assert!(again.contains(r#""plain \n \t text""#) && !again.contains("\\u{"), "{again}");
}

#[test]
fn an_invalid_unicode_escape_fails_lowering() {
    let err = parse_and_lower_str(&plain_program(r#"(set @entity.id "\u{D800}")"#)).expect_err("surrogate");
    assert!(err.to_string().contains("not a Unicode scalar value"), "{err}");
}

#[test]
fn the_credential_administration_effects_lower_with_their_request_object_and_survive_the_formatter() {
    for (call, expected) in [
        (
            "(credential-list { service: \"llm\" } { emit: { success: LISTED } })",
            json!(["credential-list", { "service": "llm" }, { "emit": { "success": "LISTED" } }]),
        ),
        (
            "(credential-set { service: \"KeysOrbital\", envVar: \"SALT\", value: @payload.value } { emit: { success: SAVED, failure: FAILED } })",
            json!(["credential-set", { "service": "KeysOrbital", "envVar": "SALT", "value": "@payload.value" }, { "emit": { "success": "SAVED", "failure": "FAILED" } }]),
        ),
        ("(credential-remove { service: \"a\", envVar: \"B\" })", json!(["credential-remove", { "service": "a", "envVar": "B" }])),
        ("(credential-test { service: \"a\" })", json!(["credential-test", { "service": "a" }])),
        ("(credential-mode { mode: \"real\" })", json!(["credential-mode", { "mode": "real" }])),
        ("(credential-rotate)", json!(["credential-rotate"])),
    ] {
        let src = program(&format!("{call}\n        (credential-forget @config.token)"));
        assert_eq!(effects_of(&src)[0], expected, "{call}");
        let formatted = format_program(&parse_str(&src).expect("parse"));
        assert!(formatted.contains(call.split_whitespace().next().unwrap()), "{formatted}");
        assert_eq!(effects_of(&formatted), effects_of(&src), "{call}");
    }
}

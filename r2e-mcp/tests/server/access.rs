//! What a view filter is told about the member and the request
//! (`session/access.rs`): `McpMember`'s kind and requirements, `McpView`'s
//! caller (principal, subject, roles, scopes, identity), session kind and
//! request head — and a member hidden by role or scope is absent for the
//! caller who lacks it, present for the one who holds it.

use std::sync::{Arc, Mutex};

use r2e_core::http::{Method, StatusCode};
use r2e_core::prelude::*;
use r2e_core::AppBuilder;
use r2e_mcp::{AppBuilderMcpExt, McpMember, McpPrincipal, McpServer, McpView, McpViewFilter};
use r2e_security::AuthenticatedUser;
use serde_json::{json, Value};

use crate::support;

#[controller]
pub struct Desk;

#[routes]
impl Desk {
    /// Open to everyone.
    #[tool]
    async fn ping(&self) -> &'static str {
        "pong"
    }

    /// Exists for auditors only; declares the role.
    #[tool]
    #[roles("auditor")]
    async fn audit(&self, #[inject(identity)] principal: McpPrincipal) -> String {
        format!("audited:{}", principal.user.sub)
    }

    /// A resource.
    #[resource(uri = "docs://handbook")]
    async fn handbook(&self) -> &'static str {
        "rules"
    }

    /// Exists for callers holding `prompts:read` only.
    #[prompt]
    async fn brief(&self) -> &'static str {
        "be brief"
    }
}

/// Everything the filter observed about one (member, request) pair.
#[derive(Clone, Debug, PartialEq)]
struct Seen {
    name: String,
    session: Option<String>,
    is_tool: bool,
    is_resource: bool,
    is_prompt: bool,
    as_tool: Option<String>,
    roles: Vec<&'static str>,
    persistent: bool,
    principal: Option<String>,
    subject: Option<String>,
    auditor: bool,
    root: bool,
    prompts_scope: bool,
    absent_scope: bool,
    identity: Option<String>,
    post: Option<bool>,
}

/// Records what it sees; `audit` needs the `auditor` role, `brief` the
/// `prompts:read` scope. `listed` is left to its default.
#[derive(Clone, Default)]
struct Probe {
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl McpViewFilter for Probe {
    fn visible(&self, member: McpMember<'_>, view: &McpView<'_>) -> bool {
        self.seen.lock().unwrap().push(Seen {
            name: member.name().to_owned(),
            session: view.session_id().map(|id| id.to_string()),
            is_tool: member.is_tool(),
            is_resource: member.is_resource(),
            is_prompt: member.is_prompt(),
            as_tool: member.as_tool().map(|tool| tool.name.to_string()),
            roles: member.requirements().roles.to_vec(),
            persistent: view.is_persistent(),
            principal: view.principal().map(|p| p.user.sub.clone()),
            subject: view.subject().map(str::to_owned),
            auditor: view.has_role("auditor"),
            root: view.has_role("root"),
            prompts_scope: view.has_scope("prompts:read"),
            absent_scope: view.has_scope("absent"),
            identity: view.identity::<AuthenticatedUser>().map(|user| user.sub),
            post: view.parts().map(|parts| parts.method == Method::POST),
        });
        match member.name() {
            "audit" => view.has_role("auditor"),
            "brief" => view.has_scope("prompts:read"),
            _ => true,
        }
    }
}

impl Probe {
    fn first(&self, name: &str, session: Option<&str>) -> Seen {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .find(|seen| seen.name == name && seen.session.as_deref() == session)
            .cloned()
            .unwrap_or_else(|| panic!("the filter was never asked about {name} ({session:?})"))
    }
}

/// A sessionless request: no session, no caller, the POST head — and the
/// default `listed` lists whatever `visible` lets through.
#[r2e_core::test]
async fn stateless_lists_use_the_default_listed_and_see_no_caller() {
    let probe = Probe::default();
    let router = AppBuilder::new()
        .provide(probe.clone())
        .plugin(
            McpServer::new()
                .stateless(true)
                .json_response(true)
                .view_filter::<Probe>(),
        )
        .build_state()
        .await
        .register_mcp_service::<Desk>()
        .build();

    let list = |method: &'static str| {
        let router = router.clone();
        async move {
            let response = support::post_with_headers(
                &router,
                "/mcp",
                None,
                &[],
                &json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": {} }),
            )
            .await;
            assert_eq!(response.status, StatusCode::OK, "{}", response.raw_body);
            response.message().clone()
        }
    };
    let names = |message: &Value, array: &str, key: &str| -> Vec<String> {
        let mut names: Vec<String> = message["result"][array]
            .as_array()
            .unwrap_or_else(|| panic!("no `{array}` in {message}"))
            .iter()
            .map(|item| item[key].as_str().unwrap().to_owned())
            .collect();
        names.sort();
        names
    };
    // No role: `audit` is hidden; `listed` (default) keeps the rest.
    assert_eq!(names(&list("tools/list").await, "tools", "name"), ["ping"]);
    assert_eq!(
        names(&list("resources/list").await, "resources", "uri"),
        ["docs://handbook"]
    );
    assert!(names(&list("prompts/list").await, "prompts", "name").is_empty());

    let ping = probe.first("ping", None);
    assert_eq!(
        ping,
        Seen {
            name: "ping".into(),
            session: None,
            is_tool: true,
            is_resource: false,
            is_prompt: false,
            as_tool: Some("ping".into()),
            roles: vec![],
            persistent: false,
            principal: None,
            subject: None,
            auditor: false,
            root: false,
            prompts_scope: false,
            absent_scope: false,
            identity: None,
            post: Some(true),
        }
    );
}

#[cfg(feature = "loopback")]
#[r2e_core::test]
async fn the_view_reports_the_caller_and_gates_on_its_roles_and_scopes() {
    use r2e_mcp::loopback::LoopbackIdentity;
    use r2e_mcp::McpLoopback;
    use r2e_security::StandardClaims;
    use rmcp::model::{CallToolRequestParams, GetPromptRequestParams};

    fn trusted(sub: &str, roles: &[&str], scopes: &[&str]) -> McpPrincipal {
        let claims = StandardClaims {
            sub: sub.to_string(),
            roles: Some(roles.iter().map(|r| r.to_string()).collect()),
            ..Default::default()
        };
        McpPrincipal::trusted(
            AuthenticatedUser::from_claims(claims),
            scopes.iter().map(|s| s.to_string()),
        )
    }

    let probe = Probe::default();
    let app = AppBuilder::new()
        .provide(probe.clone())
        .plugin(McpServer::new().view_filter::<Probe>())
        .build_state()
        .await
        .register_mcp_service::<Desk>();
    let loopback = app.bean_context().try_get::<McpLoopback>().unwrap();
    let _router = app.build();

    let alice = loopback
        .connect(
            "alice",
            LoopbackIdentity::Principal(trusted("alice", &["auditor"], &["prompts:read"])),
        )
        .await
        .unwrap();
    let bob = loopback
        .connect("bob", LoopbackIdentity::Principal(trusted("bob", &[], &[])))
        .await
        .unwrap();

    let tools = |tools: Vec<rmcp::model::Tool>| {
        let mut names: Vec<String> = tools.into_iter().map(|t| t.name.to_string()).collect();
        names.sort();
        names
    };
    let prompts = |prompts: Vec<rmcp::model::Prompt>| -> Vec<String> {
        prompts.into_iter().map(|p| p.name.to_string()).collect()
    };
    let resources = |resources: Vec<rmcp::model::Resource>| -> Vec<String> {
        resources.into_iter().map(|r| r.uri.to_string()).collect()
    };

    // Alice holds the role and the scope: everything exists and is listed.
    assert_eq!(
        tools(alice.list_all_tools().await.unwrap()),
        ["audit", "ping"]
    );
    assert_eq!(prompts(alice.list_all_prompts().await.unwrap()), ["brief"]);
    assert_eq!(
        resources(alice.list_all_resources().await.unwrap()),
        ["docs://handbook"]
    );
    assert!(alice
        .call_tool(CallToolRequestParams::new("audit"))
        .await
        .is_ok());
    assert!(alice
        .get_prompt(GetPromptRequestParams::new("brief"))
        .await
        .is_ok());

    // Bob lacks both: hidden from the lists and refused by name.
    assert_eq!(tools(bob.list_all_tools().await.unwrap()), ["ping"]);
    assert!(prompts(bob.list_all_prompts().await.unwrap()).is_empty());
    assert!(bob
        .call_tool(CallToolRequestParams::new("audit"))
        .await
        .is_err());
    assert!(bob
        .get_prompt(GetPromptRequestParams::new("brief"))
        .await
        .is_err());

    let caller = |seen: &Seen| {
        (
            seen.persistent,
            seen.principal.clone(),
            seen.subject.clone(),
            seen.auditor,
            seen.root,
            seen.prompts_scope,
            seen.absent_scope,
            seen.identity.clone(),
            seen.post,
        )
    };
    let alice_ping = probe.first("ping", Some("alice"));
    assert_eq!(
        caller(&alice_ping),
        (
            true,
            Some("alice".into()),
            Some("alice".into()),
            true,
            false,
            true,
            false,
            Some("alice".into()),
            Some(true),
        )
    );
    assert_eq!(
        caller(&probe.first("ping", Some("bob"))),
        (
            true,
            Some("bob".into()),
            Some("bob".into()),
            false,
            false,
            false,
            false,
            Some("bob".into()),
            Some(true),
        )
    );

    // The member's kind, as each family reports it.
    let kind = |seen: Seen| {
        (
            seen.is_tool,
            seen.is_resource,
            seen.is_prompt,
            seen.as_tool,
            seen.roles,
        )
    };
    assert_eq!(
        kind(alice_ping),
        (true, false, false, Some("ping".into()), vec![])
    );
    assert_eq!(
        kind(probe.first("audit", Some("alice"))),
        (true, false, false, Some("audit".into()), vec!["auditor"])
    );
    assert_eq!(
        kind(probe.first("docs://handbook", Some("alice"))),
        (false, true, false, None, vec![])
    );
    assert_eq!(
        kind(probe.first("brief", Some("alice"))),
        (false, false, true, None, vec![])
    );

    alice.close().await;
    bob.close().await;
}

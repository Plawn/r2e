//! Builders for members created at runtime, for one session
//! ([`McpSession::add_tool`](crate::McpSession::add_tool),
//! [`SessionToolset::tool`](crate::SessionToolset::tool)).
//!
//! They produce the same [`ToolRoute`] / [`ResourceRoute`] / [`PromptRoute`]
//! the `#[mcp_routes]` macro emits, with the same dispatch contract: scope
//! AND role requirements are checked before the handler runs, typed
//! arguments are deserialized through [`ToolParams`], and the handler's
//! return goes through [`IntoToolResult`] / [`IntoResourceResult`] /
//! [`IntoPromptResult`].
//!
//! ```ignore
//! session.add_tool(
//!     DynamicTool::new(format!("query_{table}"))
//!         .description("Query one discovered table")
//!         .scopes(&["db:read"])
//!         .handler(move |p: QueryIn, _call: ToolCall| async move { run(p).await }),
//! )?;
//! ```

use std::borrow::Cow;
use std::future::Future;
use std::marker::PhantomData;
use std::sync::Arc;

use r2e_core::http::Parts;
use schemars::JsonSchema;
use serde_json::Value;

use crate::auth::tools::{check_access, ToolRequirements};
use crate::catalog::principal_in;
use crate::error::McpError;
use crate::params::{
    empty_object_schema, prompt_arguments_from_schema, schema_object_for, ToolParams,
};
use crate::result::{IntoPromptResult, IntoResourceResult, IntoToolResult};
use crate::route::{
    PromptArgumentDef, PromptCall, PromptInvoke, PromptRoute, ResourceCall, ResourceInvoke,
    ResourceRoute, SchemaObject, ToolAnnotations, ToolCall, ToolInvoke, ToolRoute,
};

/// The four requirement setters every dynamic builder shares (its
/// `requirements: ToolRequirements` field).
macro_rules! requirement_setters {
    () => {
        /// Scopes the caller must all hold (requires `mcp.auth`).
        pub fn scopes(mut self, scopes: &'static [&'static str]) -> Self {
            self.requirements.scopes = scopes;
            self
        }

        /// Scopes of which the caller must hold at least one.
        pub fn any_scopes(mut self, scopes: &'static [&'static str]) -> Self {
            self.requirements.any_scopes = scopes;
            self
        }

        /// Roles of which the caller must hold at least one.
        pub fn roles(mut self, roles: &'static [&'static str]) -> Self {
            self.requirements.roles = roles;
            self
        }

        /// Roles the caller must all hold.
        pub fn all_roles(mut self, roles: &'static [&'static str]) -> Self {
            self.requirements.all_roles = roles;
            self
        }
    };
}

/// Handler marker: the closure takes only the call context.
pub struct NoParams;

/// Handler marker: the closure takes typed arguments `P` and the call.
pub struct WithParams<P>(PhantomData<fn() -> P>);

/// Scope check (as the macro emits it) followed by the role check the macro
/// delegates to its guard — dynamic members have no guard, so roles are
/// enforced here.
fn check_member_access(
    parts: Option<&Parts>,
    kind: &'static str,
    name: &str,
    req: &ToolRequirements,
) -> Result<(), McpError> {
    check_access(parts.map(|p| &p.extensions), kind, name, req)?;
    if req.roles.is_empty() && req.all_roles.is_empty() {
        return Ok(());
    }
    let Some(principal) = principal_in(parts) else {
        return Err(McpError::Unauthorized(format!(
            "{kind} `{name}` requires an authenticated caller"
        )));
    };
    if req.roles_ok(principal) {
        Ok(())
    } else {
        Err(McpError::Forbidden(format!(
            "{kind} `{name}` requires a role the caller does not hold"
        )))
    }
}

/// A tool handler closure; implemented for `Fn(ToolCall) -> Fut` and
/// `Fn(P, ToolCall) -> Fut` (annotate the closure parameters so the arity
/// is inferred).
pub trait DynamicToolHandler<M>: Send + Sync + 'static {
    #[doc(hidden)]
    fn input_schema() -> SchemaObject;
    #[doc(hidden)]
    fn into_invoke(self, name: Arc<str>, requirements: ToolRequirements) -> ToolInvoke;
}

impl<F, Fut, R> DynamicToolHandler<NoParams> for F
where
    F: Fn(ToolCall) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = R> + Send + 'static,
    R: IntoToolResult,
{
    fn input_schema() -> SchemaObject {
        empty_object_schema()
    }

    fn into_invoke(self, name: Arc<str>, requirements: ToolRequirements) -> ToolInvoke {
        let handler = Arc::new(self);
        Arc::new(move |call: ToolCall| {
            let handler = Arc::clone(&handler);
            let name = Arc::clone(&name);
            Box::pin(async move {
                check_member_access(call.parts.as_deref(), "tool", &name, &requirements)?;
                handler(call).await.into_tool_result()
            })
        })
    }
}

impl<F, Fut, R, P> DynamicToolHandler<WithParams<P>> for F
where
    F: Fn(P, ToolCall) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = R> + Send + 'static,
    R: IntoToolResult,
    P: ToolParams + Send + 'static,
{
    fn input_schema() -> SchemaObject {
        P::input_schema()
    }

    fn into_invoke(self, name: Arc<str>, requirements: ToolRequirements) -> ToolInvoke {
        let handler = Arc::new(self);
        Arc::new(move |mut call: ToolCall| {
            let handler = Arc::clone(&handler);
            let name = Arc::clone(&name);
            Box::pin(async move {
                check_member_access(call.parts.as_deref(), "tool", &name, &requirements)?;
                let arguments = std::mem::replace(&mut call.arguments, Value::Null);
                let params = P::from_arguments(arguments)?;
                handler(params, call).await.into_tool_result()
            })
        })
    }
}

/// Builder for a session-private tool.
#[must_use]
pub struct DynamicTool {
    name: Cow<'static, str>,
    title: Option<String>,
    description: Option<Cow<'static, str>>,
    output_schema: Option<Arc<SchemaObject>>,
    annotations: ToolAnnotations,
    requirements: ToolRequirements,
}

impl DynamicTool {
    /// Start a tool named `name`.
    pub fn new(name: impl Into<Cow<'static, str>>) -> Self {
        DynamicTool {
            name: name.into(),
            title: None,
            description: None,
            output_schema: None,
            annotations: ToolAnnotations::default(),
            requirements: ToolRequirements::NONE,
        }
    }

    /// Display title.
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Description shown to the model.
    pub fn description(mut self, description: impl Into<Cow<'static, str>>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Behavioral hints (read-only, destructive, …).
    pub fn annotations(mut self, annotations: ToolAnnotations) -> Self {
        self.annotations = annotations;
        self
    }

    /// Advertise `T`'s schema as the tool's `outputSchema`.
    pub fn output<T: JsonSchema>(mut self) -> Self {
        self.output_schema = Some(Arc::new(schema_object_for::<T>()));
        self
    }

    requirement_setters!();

    /// Finish with the handler: `|call: ToolCall| async { … }` or
    /// `|p: MyParams, call: ToolCall| async { … }` (the input schema comes
    /// from `MyParams`).
    pub fn handler<M, H: DynamicToolHandler<M>>(self, handler: H) -> ToolRoute {
        let invoke = handler.into_invoke(Arc::from(self.name.as_ref()), self.requirements);
        ToolRoute {
            name: self.name,
            title: self.title,
            description: self.description,
            input_schema: Arc::new(H::input_schema()),
            output_schema: self.output_schema,
            annotations: self.annotations,
            requirements: self.requirements,
            group: None,
            invoke,
        }
    }
}

/// Builder for a session-private resource (fixed URI or RFC 6570 template —
/// captured variables arrive in [`ResourceCall::variables`]).
#[must_use]
pub struct DynamicResource {
    uri: Cow<'static, str>,
    name: Cow<'static, str>,
    title: Option<String>,
    description: Option<String>,
    mime_type: Option<String>,
    requirements: ToolRequirements,
}

impl DynamicResource {
    /// Start a resource served at `uri`, listed as `name`.
    pub fn new(uri: impl Into<Cow<'static, str>>, name: impl Into<Cow<'static, str>>) -> Self {
        DynamicResource {
            uri: uri.into(),
            name: name.into(),
            title: None,
            description: None,
            mime_type: None,
            requirements: ToolRequirements::NONE,
        }
    }

    /// Display title.
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Description.
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// MIME type of text-shaped returns.
    pub fn mime_type(mut self, mime_type: impl Into<String>) -> Self {
        self.mime_type = Some(mime_type.into());
        self
    }

    requirement_setters!();

    /// Finish with the read handler.
    pub fn handler<F, Fut, R>(self, handler: F) -> ResourceRoute
    where
        F: Fn(ResourceCall) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: IntoResourceResult,
    {
        let handler = Arc::new(handler);
        let name: Arc<str> = Arc::from(self.uri.as_ref());
        let mime: Option<Arc<str>> = self.mime_type.as_deref().map(Arc::from);
        let requirements = self.requirements;
        let invoke: ResourceInvoke = Arc::new(move |call: ResourceCall| {
            let handler = Arc::clone(&handler);
            let name = Arc::clone(&name);
            let mime = mime.clone();
            Box::pin(async move {
                check_member_access(call.parts.as_deref(), "resource", &name, &requirements)?;
                let uri = call.uri.clone();
                handler(call)
                    .await
                    .into_resource_result(&uri, mime.as_deref())
            })
        });
        ResourceRoute {
            uri: self.uri,
            name: self.name,
            title: self.title,
            description: self.description,
            mime_type: self.mime_type,
            requirements,
            group: None,
            invoke,
        }
    }
}

/// A prompt handler closure; implemented for `Fn(PromptCall) -> Fut` and
/// `Fn(P, PromptCall) -> Fut`.
pub trait DynamicPromptHandler<M>: Send + Sync + 'static {
    #[doc(hidden)]
    fn arguments() -> Vec<PromptArgumentDef>;
    #[doc(hidden)]
    fn into_invoke(
        self,
        name: Arc<str>,
        description: Option<Arc<str>>,
        requirements: ToolRequirements,
    ) -> PromptInvoke;
}

impl<F, Fut, R> DynamicPromptHandler<NoParams> for F
where
    F: Fn(PromptCall) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = R> + Send + 'static,
    R: IntoPromptResult,
{
    fn arguments() -> Vec<PromptArgumentDef> {
        Vec::new()
    }

    fn into_invoke(
        self,
        name: Arc<str>,
        description: Option<Arc<str>>,
        requirements: ToolRequirements,
    ) -> PromptInvoke {
        let handler = Arc::new(self);
        Arc::new(move |call: PromptCall| {
            let handler = Arc::clone(&handler);
            let name = Arc::clone(&name);
            let description = description.clone();
            Box::pin(async move {
                check_member_access(call.parts.as_deref(), "prompt", &name, &requirements)?;
                handler(call)
                    .await
                    .into_prompt_result(description.as_deref())
            })
        })
    }
}

impl<F, Fut, R, P> DynamicPromptHandler<WithParams<P>> for F
where
    F: Fn(P, PromptCall) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = R> + Send + 'static,
    R: IntoPromptResult,
    P: ToolParams + Send + 'static,
{
    fn arguments() -> Vec<PromptArgumentDef> {
        prompt_arguments_from_schema(&P::input_schema())
    }

    fn into_invoke(
        self,
        name: Arc<str>,
        description: Option<Arc<str>>,
        requirements: ToolRequirements,
    ) -> PromptInvoke {
        let handler = Arc::new(self);
        Arc::new(move |mut call: PromptCall| {
            let handler = Arc::clone(&handler);
            let name = Arc::clone(&name);
            let description = description.clone();
            Box::pin(async move {
                check_member_access(call.parts.as_deref(), "prompt", &name, &requirements)?;
                let arguments = std::mem::replace(&mut call.arguments, Value::Null);
                let params = P::from_arguments(arguments)?;
                handler(params, call)
                    .await
                    .into_prompt_result(description.as_deref())
            })
        })
    }
}

/// Builder for a session-private prompt.
#[must_use]
pub struct DynamicPrompt {
    name: Cow<'static, str>,
    title: Option<String>,
    description: Option<String>,
    requirements: ToolRequirements,
}

impl DynamicPrompt {
    /// Start a prompt named `name`.
    pub fn new(name: impl Into<Cow<'static, str>>) -> Self {
        DynamicPrompt {
            name: name.into(),
            title: None,
            description: None,
            requirements: ToolRequirements::NONE,
        }
    }

    /// Display title.
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Description.
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    requirement_setters!();

    /// Finish with the handler: `|call: PromptCall| async { … }` or
    /// `|p: MyArgs, call: PromptCall| async { … }` (the advertised arguments
    /// come from `MyArgs`).
    pub fn handler<M, H: DynamicPromptHandler<M>>(self, handler: H) -> PromptRoute {
        let invoke = handler.into_invoke(
            Arc::from(self.name.as_ref()),
            self.description.as_deref().map(Arc::from),
            self.requirements,
        );
        PromptRoute {
            name: self.name,
            title: self.title,
            description: self.description,
            arguments: H::arguments(),
            requirements: self.requirements,
            group: None,
            invoke,
        }
    }
}

use serde::Serialize;
use serde_json::Value;

/// Metadata about a single route, collected at compile time.
#[derive(Debug, Clone, Serialize)]
pub struct RouteInfo {
    pub path: String,
    pub method: String,
    pub operation_id: String,
    pub summary: Option<String>,
    pub request_body_type: Option<String>,
    pub request_body_schema: Option<Value>,
    pub response_type: Option<String>,
    pub params: Vec<ParamInfo>,
    pub roles: Vec<String>,
    pub tag: Option<String>,
}

/// Metadata about a route parameter.
#[derive(Debug, Clone, Serialize)]
pub struct ParamInfo {
    pub name: String,
    pub location: ParamLocation,
    pub param_type: String,
    pub required: bool,
}

/// Where a parameter is located in the HTTP request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ParamLocation {
    Path,
    Query,
    Header,
}

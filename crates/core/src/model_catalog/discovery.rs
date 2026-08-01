use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredModel {
    pub id: String,
    pub endpoint_id: String,
    pub region: String,
    #[serde(default)]
    pub display_name: Option<String>,
}

impl DiscoveredModel {
    pub fn new(
        id: impl Into<String>,
        endpoint_id: impl Into<String>,
        region: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            endpoint_id: endpoint_id.into(),
            region: region.into(),
            display_name: None,
        }
    }
}

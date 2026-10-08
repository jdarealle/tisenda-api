use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct ModelIdentity {
    pub(super) revision: String,
    pub(super) dtype: String,
}

// Add #![allow(unused_imports)] to root project if needed

use serde::{Deserialize, Serialize};
use serde_repr::{Deserialize_repr, Serialize_repr};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct UserAdd {
    pub email: String,
    pub alias: String,
    #[serde(rename = "firstName")]
    pub first_name: String,
    #[serde(rename = "lastName")]
    pub last_name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct UserAddResponse {
    pub id: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct UserRemove {
    pub alias: String,
}
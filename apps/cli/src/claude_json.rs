//! Parses ~/.claude.json. (Full implementation in Task 2.)

use std::path::Path;

#[derive(Clone, Debug, PartialEq)]
pub struct OAuthAccount {
    pub account_uuid: String,
    pub organization_uuid: Option<String>,
    pub email_address: Option<String>,
    pub organization_type: Option<String>,
    pub organization_rate_limit_tier: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClaudeState {
    pub account: Option<OAuthAccount>,
}

pub fn read(_path: &Path) -> ClaudeState {
    ClaudeState::default()
}

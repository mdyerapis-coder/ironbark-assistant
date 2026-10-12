use super::PolicyError;
use crate::contracts::{ActionSpec, ApprovalGrant, ProjectPolicy};

pub(super) fn validate(
    action: &ActionSpec,
    grant: &ApprovalGrant,
    policy: &ProjectPolicy,
    now_unix_ms: i64,
) -> std::result::Result<(), PolicyError> {
    if grant.revoked_at.is_some() {
        return Err(PolicyError::RevokedGrant);
    }
    if grant.expires_at <= now_unix_ms {
        return Err(PolicyError::ExpiredGrant);
    }
    if grant.policy_version != policy.version {
        return Err(PolicyError::PolicyVersionMismatch);
    }
    if grant.action_id != action.id
        || grant.expected_state_digest != action.expected_state_digest
        || action.digest().map_err(|_| PolicyError::InvalidAction)? != grant.action_digest {
        return Err(PolicyError::StaleGrant);
    }
    Ok(())
}

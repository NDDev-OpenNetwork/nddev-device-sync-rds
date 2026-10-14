//! Shared actual native grant verification for native and WASM adapters.
//! Caller supplies a current signed NDS projection and its actual endpoint key.
use super::{Result, require, response};
use nddev_device_sync_domain::screen::ScreenError;
use nddev_device_sync_domain::screen::{MAX_VIEW_MS, NativeGrantFacts, ViewProjection};
use rds_core::{ServiceKind, grant::Grant};
use std::{collections::HashSet, time::Duration};

/// Actual native crypto followed by Core's exact context matcher. Shared by
/// viewer and host; no passive `verified=true` input or external grant issuer.
pub fn verify_native(
    view: &mut ViewProjection,
    grant: &Grant,
    subject: &[u8; 32],
    now_ms: u64,
) -> Result<()> {
    let permit = view.permit();
    require(subject == &permit.viewer_endpoint, ScreenError::Denied)?;
    let checked = grant
        .verify(
            &HashSet::from([permit.host_endpoint]),
            subject,
            &permit.host_endpoint,
            Duration::from_millis(MAX_VIEW_MS),
            now_ms / 1000,
        )
        .map_err(|error| self::error(&error))?;
    let display = permit.lease.display();
    view.match_native_grant(
        &NativeGrantFacts {
            issuer: checked.payload.issuer,
            subject: checked.payload.subject,
            audience: checked.payload.audience,
            expires_at_ms: checked
                .payload
                .expires_at
                .checked_mul(1000)
                .ok_or_else(|| response(()))?,
            tenant: checked.tenant(),
            policy_revision: checked.policy_revision(),
            desktop_view: checked.permits_desktop_view(),
            desktop_control: checked.permits_desktop_control(),
            other_services: checked
                .payload
                .services
                .iter()
                .any(|s| *s != ServiceKind::DesktopView),
            display_allowed: checked.permits_display(display),
        },
        now_ms,
    )
}

/// Redacted native grant failures; no native malformed text or fields escape.
pub(super) fn error(error: &rds_core::grant::GrantError) -> ScreenError {
    use rds_core::grant::GrantError;
    match error {
        GrantError::Expired => ScreenError::Expired,
        GrantError::Malformed(_)
        | GrantError::Version
        | GrantError::InvalidInterval
        | GrantError::InvalidRevision
        | GrantError::Oversized => response(()),
        _ => ScreenError::Denied,
    }
}

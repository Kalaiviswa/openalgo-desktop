import sys,re
src=open(sys.argv[1]).read()
def rep(old,new,count=1):
    global src
    assert old in src, old[:90]
    src=src.replace(old,new,count)
def cut(a_mark,b_mark,new):
    global src
    a=src.index(a_mark); b=src.index(b_mark)
    src=src[:a]+new+src[b:]
rep('''/// What a sign-in answers when the password was changed while it was being
/// checked.
pub const PASSWORD_CHANGED_DURING_SIGN_IN: &str =
    "Your password was just changed. Sign in again with the new password.";''','''/// What a sign-in answers when the password was changed, or every window
/// was signed out, while it was being checked.
pub const PASSWORD_CHANGED_DURING_SIGN_IN: &str =
    "You were signed out, or your password was changed, while signing in. Sign in again.";''')
rep('''/// Issue the signed-in session. `epoch` is the credentials generation read
/// before the account was read: if a password change or reset (or an
/// account reset) happened since, no session is issued, even though the
/// password checked out against the hash read before the change
/// (`SecurityManager::issue_if_current`; security review of SEC-02).
async fn finish_sign_in(
    ctx: &Arc<AppState>,
    old_session: Option<&str>,
    username: &str,
    epoch: u64,
) -> Response {
    let now = ctx.now();
    let issued = ctx.security.issue_if_current(epoch, || {''','''/// Issue the signed-in session and install the keys the password opened,
/// together (KEY-01). `epoch` is the credentials generation read before the
/// account was read: if a password change or reset, an account reset or a
/// logout happened since, no session is issued and nothing is installed,
/// even though the password checked out against the hash read before the
/// change (`SecurityManager::issue_if_current`; security review of SEC-02).
async fn finish_sign_in(
    ctx: &Arc<AppState>,
    old_session: Option<&str>,
    username: &str,
    epoch: u64,
    keys: crate::security::Unlocked,
) -> Response {
    let now = ctx.now();
    let issued = ctx.security.issue_if_current(epoch, || {
        ctx.security.install(&keys);''')
rep('''            x.pending_totp_started = None;
            x.pending_epoch = None;
        });
        s
    });''','''            x.pending_totp_started = None;
            x.pending_epoch = None;
            x.pending_keys = None;
        });
        s
    });''')
rep('''        Err(e) => {
            tracing::warn!("Could not resume the broker session: {}", e);
            json!({"status": "success", "redirect": "/broker"})
        }
    };
    with_cookie(ok(body), &s.id)''','''        Err(e) => {
            tracing::warn!("Could not resume the broker session: {}", e);
            json!({"status": "success", "redirect": "/broker"})
        }
    };
    // Telegram and WhatsApp come back with the keys, as the web's start-up
    // brings them back (they were stopped at the logout).
    crate::messaging::autostart(ctx);
    with_cookie(ok(body), &s.id)''')
rep('''            finish_sign_in(&ctx, sess.as_ref().map(|s| s.id.as_str()), &user, epoch).await
        }
        Ok(Ok(LoginOutcome::TotpRequired(user))) => {''','''            finish_sign_in(
                &ctx,
                sess.as_ref().map(|s| s.id.as_str()),
                &user,
                epoch,
                keys,
            )
            .await
        }
        Ok(Ok(LoginOutcome::TotpRequired(user, keys))) => {''')
rep('''        Ok(Ok(LoginOutcome::Success(user))) => {''','''        Ok(Ok(LoginOutcome::Success(user, keys))) => {''')
rep('''                // The code step issues the session only if the password is
                // still the one checked here.
                x.pending_epoch = Some(epoch);''','''                // The code step issues the session only if the password is
                // still the one checked here.
                x.pending_epoch = Some(epoch);
                // The keys the password opened, used only to check the
                // code and installed only with the session (KEY-01).
                x.pending_keys = Some(keys);''')
rep('''        ctx.sessions.update(&s.id, |x| {
            x.pending_totp_user = None;
            x.pending_totp_started = None;
            x.pending_epoch = None;
        });
        return json_response(''','''        ctx.sessions.update(&s.id, |x| {
            x.pending_totp_user = None;
            x.pending_totp_started = None;
            x.pending_epoch = None;
            x.pending_keys = None;
        });
        return json_response(''')
rep('''    let Some(user) = s.pending_totp_user.clone() else {
        return error(StatusCode::UNAUTHORIZED, "No pending login. Sign in first.");
    };''','''    let (Some(user), Some(keys)) = (s.pending_totp_user.clone(), s.pending_keys.clone()) else {
        return error(StatusCode::UNAUTHORIZED, "No pending login. Sign in first.");
    };''')
rep('''    match AuthService::verify_totp_for(&ctx, &user, &code) {''','''    match AuthService::verify_totp_with(&ctx, &user, &code, &keys) {''')
rep('''            let epoch = s.pending_epoch.unwrap_or(u64::MAX);
            finish_sign_in(&ctx, Some(&s.id), &user, epoch).await''','''            let epoch = s.pending_epoch.unwrap_or(u64::MAX);
            finish_sign_in(&ctx, Some(&s.id), &user, epoch, keys).await''')
# session status
rep('''    let key_mode = ctx.security.mode();''','''    // The keys are always locked with the password now (KEY-01); `locked`
    // says whether they are in memory.
    let key_mode = "password";
    let locked = !ctx.security.is_unlocked();''')
src=src.replace('"key_mode": key_mode,','"key_mode": key_mode, "locked": locked,')
# logout: lock everything after revoke
rep('''        ctx.bus.publish(Event::ForceLogout {
            message: "You have been logged out from another device.".into(),
        });
        tracing::info!("Signed out");''','''        // The keys and everything decrypted with them leave memory, and
        // every cache is cleared (KEY-01).
        AuthService::lock_everything(&ctx).await;
        ctx.bus.publish(Event::ForceLogout {
            message: "You have been logged out from another device.".into(),
        });
        tracing::info!("Signed out");''')
# remove reset_password route and hash_token helper
cut('''fn hash_token(t: &str) -> String {''','''/// POST /auth/change-password''','')
open(sys.argv[2],'w').write(src)

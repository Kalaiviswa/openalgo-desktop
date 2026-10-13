
/// The answer the new-password form gets back.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct StartFreshOutcome {
    /// `done` or `cancelled`.
    pub status: &'static str,
    pub message: String,
}

/// A new password without the old one (KEY-01): keeps the account, its
/// settings and history, and drops what the old keys protected (saved
/// broker keys, the API key, two-factor login), for a forgotten password or
/// an earlier build's keychain that cannot be read. Only the person at this
/// computer can do it: the call must come from the main window on the
/// app's own page, and the trader confirms in a native dialog page scripts
/// cannot click. No HTTP route reaches it, so nothing over the network (a
/// LAN device, a tunnel, a web page) can replace the password.
#[tauri::command]
pub async fn start_fresh(
    app: AppHandle,
    window: tauri::WebviewWindow,
    shell: State<'_, ShellState>,
    new_password: String,
    confirm_password: String,
    reason: String,
) -> Result<StartFreshOutcome> {
    let url = window
        .url()
        .map_err(|e| AppError::Internal(format!("window address: {}", e)))?;
    if !reset_caller_allowed(window.label(), &url, shell.ctx.live_port()) {
        tracing::warn!("Fresh start refused: not requested from the OpenAlgo window");
        return Err(AppError::Auth(
            "A new password can be set only from the OpenAlgo Desktop window on this computer."
                .into(),
        ));
    }
    let new_password = crate::security::Secret::new(new_password);
    let confirm_password = crate::security::Secret::new(confirm_password);
    // Checked before the dialog, so the trader is not asked to confirm a
    // password that would be refused.
    if new_password.expose() != confirm_password.expose() {
        return Err(AppError::Validation("The passwords do not match.".into()));
    }
    crate::services::auth_service::validate_password_strength(new_password.expose())
        .map_err(|m| AppError::Validation(m.into()))?;
    let forgot = reason == "forgot";
    let (tx, rx) = tokio::sync::oneshot::channel();
    {
        use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
        app.dialog()
            .message(
                "Your account, settings, strategies and history stay. Your saved broker keys, \
your API key and two-factor login cannot be opened without the old password, so they are \
removed: afterwards you enter your broker keys again, copy your new API key into the programs \
that use it, and set up two-factor login again. Telegram and WhatsApp are set up again too.",
            )
            .title(if forgot {
                "Set a new password?"
            } else {
                "Start fresh with a new password?"
            })
            .kind(MessageDialogKind::Warning)
            .buttons(MessageDialogButtons::OkCancelCustom(
                if forgot {
                    "Set new password".into()
                } else {
                    "Start fresh".into()
                },
                "Keep everything as it is".into(),
            ))
            .parent(&window)
            .show(move |confirmed| {
                let _ = tx.send(confirmed);
            });
    }
    if !rx.await.unwrap_or(false) {
        return Ok(StartFreshOutcome {
            status: "cancelled",
            message: "Nothing was changed.".into(),
        });
    }
    crate::services::auth_service::AuthService::start_fresh_everywhere(
        &shell.ctx,
        new_password.expose(),
        confirm_password.expose(),
    )
    .await?;
    tracing::info!("New password set from the desktop window");
    Ok(StartFreshOutcome {
        status: "done",
        message: "Your new password is set. Sign in with it, then enter your broker keys again \
on Broker Configuration and copy your new API key from the API Key page."
            .into(),
    })
}

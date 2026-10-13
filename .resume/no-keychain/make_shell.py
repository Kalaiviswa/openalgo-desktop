import sys,os
root=sys.argv[1]  # src-tauri
def edit(rel, pairs):
    p=os.path.join(root,rel); s=open(p).read()
    for old,new in pairs:
        assert old in s, (rel, old[:70]); s=s.replace(old,new,1)
    open(p,'w').write(s)
edit('build.rs',[('''        "reset_account",
    ]);''','''        "reset_account",
        "start_fresh",
    ]);''')])
edit('src/lib.rs',[('''                commands::reset_account,
            ])''','''                commands::reset_account,
                commands::start_fresh,
            ])'''),('''                    feed.start().await;
                    messaging::autostart(&ctx);
                    Ok::<_, error::AppError>((ctx, server, feed))''','''                    feed.start().await;
                    // Telegram and WhatsApp start at the first sign-in, when
                    // the keys that open their tokens are in memory (KEY-01).
                    Ok::<_, error::AppError>((ctx, server, feed))''')])
start_fresh=open(os.path.join(os.path.dirname(os.path.abspath(sys.argv[0])),'commands_start_fresh.rs')).read()
edit('src/commands/mod.rs',[('''//! | `reset_account`  | the main window on the app's own page, after a native confirmation | last-resort recovery when the password and authenticator are both lost |''','''//! | `reset_account`  | the main window on the app's own page, after a native confirmation | start over with a new account |
//! | `start_fresh`    | the main window on the app's own page, after a native confirmation | a new password without the old one: forgotten, or an earlier build's keys unreadable (KEY-01) |'''),
('''pub const REMOTE_APP_PERMISSIONS: [&str; 8] = [''','''pub const REMOTE_APP_PERMISSIONS: [&str; 9] = ['''),
('''    "allow-reset-account",
];''','''    "allow-reset-account",
    "allow-start-fresh",
];'''),
('''#[cfg(test)]
mod reset_tests {''',start_fresh.lstrip('\n')+'''
#[cfg(test)]
mod reset_tests {''')])

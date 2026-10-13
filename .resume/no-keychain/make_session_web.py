import sys
p=sys.argv[1]
s=open(p).read()
def rep(old,new):
    global s
    assert old in s, old[:80]
    s=s.replace(old,new,1)
rep('''    /// SHA-256 of the password-reset token (the raw token is never stored).
    pub reset_token_hash: Option<String>,
    pub reset_email: Option<String>,''','''    /// The keys a sign-in's password opened while it waits for the
    /// authenticator code: used only to check the code, installed only with
    /// the signed-in session, dropped (and zeroed) when this session ends
    /// (KEY-01).
    pub pending_keys: Option<crate::security::Unlocked>,''')
open(p,'w').write(s)

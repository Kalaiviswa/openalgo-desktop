import sys
p=sys.argv[1]
s=open(p).read()
def rep(old,new):
    global s
    assert old in s, old[:80]
    s=s.replace(old,new,1)
rep('''        self.sandbox.shutdown().await;
        self.bus.shutdown(Duration::from_secs(2)).await;
        let mut tasks = std::mem::take(&mut *self.tasks.lock());
        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
    }''','''        self.sandbox.shutdown().await;
        self.bus.shutdown(Duration::from_secs(2)).await;
        let mut tasks = std::mem::take(&mut *self.tasks.lock());
        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
        // Quitting wipes the keys like a logout (KEY-01).
        self.security.lock();
    }''')
rep('''    pub fn get_broker_session(&self) -> Option<BrokerSession> {
        let s = self.broker_session.read().clone()?;
        let cfg = self.config.read();
        if crate::session::boundary::is_fresh(
            s.authenticated_at,
            self.clock.now(),
            cfg.session_expiry_hour,
            cfg.session_expiry_minute,
        ) {
            Some(s)
        } else {
            drop(cfg);
            *self.broker_session.write() = None;
            None
        }
    }''','''    pub fn get_broker_session(&self) -> Option<BrokerSession> {
        let s = self.broker_session.read().clone()?;
        let cfg = self.config.read();
        if crate::session::boundary::is_fresh(
            s.authenticated_at,
            self.clock.now(),
            cfg.session_expiry_hour,
            cfg.session_expiry_minute,
        ) {
            Some(s)
        } else {
            drop(cfg);
            // Clear only the session this read found stale: a sign-in that
            // set a new one in between keeps it (KEY-01 cache rules: a
            // writer never undoes a newer value).
            let mut slot = self.broker_session.write();
            if slot.as_ref().is_some_and(|cur| {
                cur.broker_id == s.broker_id && cur.authenticated_at == s.authenticated_at
            }) {
                *slot = None;
            }
            None
        }
    }''')
open(p,'w').write(s)

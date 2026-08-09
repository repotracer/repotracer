use super::session::{create_session, revoke_session, Session};

pub fn rotate_refresh_token(session: &mut Session) -> String {
    let old = session.refresh_token.clone();
    // reuse detection would compare presented token to `old`
    let next = create_session(&session.id);
    session.refresh_token = next.refresh_token;
    let _ = old;
    session.refresh_token.clone()
}

pub fn detect_reuse(presented: &str, expected: &str) -> bool {
    presented != expected
}

pub fn revoke_all(session: &mut Session) {
    revoke_session(session);
}

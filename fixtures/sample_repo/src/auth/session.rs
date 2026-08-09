// Sample session module for offline demos / mock scouts.

pub struct Session {
    pub id: String,
    pub refresh_token: String,
}

pub fn create_session(user_id: &str) -> Session {
    Session {
        id: format!("sess_{user_id}"),
        refresh_token: format!("rt_{user_id}"),
    }
}

pub fn validate_session(session: &Session) -> bool {
    !session.id.is_empty() && !session.refresh_token.is_empty()
}

pub fn revoke_session(session: &mut Session) {
    session.refresh_token.clear();
}

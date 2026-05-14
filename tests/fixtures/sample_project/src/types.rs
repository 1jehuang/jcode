/// Sample types module for cross-file repair testing
pub struct User {
    pub id: u64,
    pub name: String,
    pub email: String,
}

impl User {
    pub fn new(id: u64, name: &str, email: &str) -> Self {
        Self { id, name: name.to_string(), email: email.to_string() }
    }
}

pub struct Order {
    pub id: u64,
    pub user_id: u64,
    pub total: f64,
}

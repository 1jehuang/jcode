mod types;

use types::{User, Order};

fn main() {
    let user = User::new(1, "Alice", "alice@example.com");
    println!("User: {} ({})", user.name, user.email);

    let order = Order { id: 1, user_id: user.id, total: 99.99 };
    println!("Order #{}: ${:.2}", order.id, order.total);
}

fn compute_total(orders: &[Order]) -> f64 {
    orders.iter().map(|o| o.total).sum()
}

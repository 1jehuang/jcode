fn main() {
    println!(
        "entry_future_bytes={}",
        std::mem::size_of_val(&jcode::run())
    );
    println!(
        "wrapped_entry_future_bytes={}",
        std::mem::size_of_val(&async { jcode::run().await })
    );
    println!(
        "bus_event_bytes={}",
        std::mem::size_of::<jcode::bus::BusEvent>()
    );
}

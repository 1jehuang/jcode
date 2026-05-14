/// Broken module referencing a missing type
pub struct Item {
    pub id: u64,
    pub name: String,
    // Reference to MissingType which doesn't exist
    pub metadata: MissingType,
}

// Function with wrong parameter type
pub fn process(item: &NonExistentStruct) -> String {
    item.name.clone()
}

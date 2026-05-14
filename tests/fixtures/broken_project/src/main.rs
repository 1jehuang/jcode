mod missing;

fn main() {
    // This should trigger type errors due to missing types
    let item = missing::Item { id: 1, name: "test".into(), metadata: missing::MissingType };
}

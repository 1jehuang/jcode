fn main() {
    // realistic model picker filter_text: "name provider method detail"
    let names: Vec<String> = (0..9000)
        .map(|i| {
            format!(
                "provider-{}/model-name-v{}-instruct-{}k OpenRouter openrouter → SomeProvider ctx 200k · in $1.25/M out $10/M · cached 3h ago",
                i % 400, i % 9, i % 128
            )
        })
        .collect();
    for q in [
        "g",
        "gp",
        "gpt",
        "gpt-5",
        "claud",
        "claude sonnet",
        "opsu",
        "codxe",
        "openrouter",
    ] {
        let prepared = jcode_fuzzy::PreparedTokenQuery::new(q);
        let start = std::time::Instant::now();
        let mut n = 0;
        for name in &names {
            if prepared.score(name).is_some() {
                n += 1;
            }
        }
        println!("{q:>14}: {:?} ({n} matches)", start.elapsed());
    }
}

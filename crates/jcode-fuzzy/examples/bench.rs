fn main() {
    let names: Vec<String> = (0..3000).map(|i| format!("provider-{i}/model-name-v{}-instruct-{}k gpt openai responses detail text here", i%9, i%128)).collect();
    let queries = ["g", "gp", "gpt", "gpt-5", "claude", "opsu", "codxe"];
    let start = std::time::Instant::now();
    let mut total = 0usize;
    for q in queries {
        for n in &names {
            if jcode_fuzzy::fuzzy_score_tokens(q, n).is_some() { total += 1; }
        }
    }
    println!("{} matches in {:?} ({} scored)", total, start.elapsed(), queries.len()*names.len());
}

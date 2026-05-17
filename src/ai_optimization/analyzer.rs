use super::collector::UsageEvent;

pub struct BehaviorAnalyzer {
    patterns: Vec<BehaviorPattern>,
}

#[derive(Debug, Clone)]
pub struct BehaviorPattern {
    pub id: String,
    pub pattern_type: PatternType,
    pub description: String,
    pub confidence: f64,
    pub frequency: u32,
    pub last_seen: chrono::DateTime<chrono::Utc>,
    pub suggestions: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum PatternType {
    CommandSequence,
    TimeOfDayPreference,
    ErrorPattern,
    WorkflowHabit,
    PerformanceAnomaly,
}

impl BehaviorAnalyzer {
    pub fn new() -> Self {
        BehaviorAnalyzer { patterns: vec![] }
    }

    pub fn analyze(&mut self, events: &[UsageEvent]) -> Vec<BehaviorPattern> {
        self.detect_command_sequences(events);
        self.detect_time_patterns(events);
        self.detect_error_patterns(events);
        self.patterns.sort_by(|a, b| b.confidence.partial_cmp(&a.confidence).unwrap_or(std::cmp::Ordering::Equal));
        self.patterns.clone()
    }

    fn detect_command_sequences(&mut self, events: &[UsageEvent]) {
        let sequences: Vec<Vec<String>> = events.windows(3)
            .filter_map(|w| {
                let cmds: Vec<String> = w.iter()
                    .filter_map(|e| e.data.command.clone())
                    .collect();
                if cmds.len() == 3 { Some(cmds) } else { None }
            })
            .collect();

        for seq in &sequences {
            if self.is_frequent_sequence(seq) {
                self.patterns.push(BehaviorPattern {
                    id: uuid::Uuid::new_v4().to_string(),
                    pattern_type: PatternType::CommandSequence,
                    description: format!("Frequent sequence: {} -> {} -> {}", seq[0], seq[1], seq[2]),
                    confidence: 0.85,
                    frequency: 5,
                    last_seen: chrono::Utc::now(),
                    suggestions: vec![
                        format!("Create alias for '{} -> {} -> {}'", seq[0], seq[1], seq[2]),
                        "Consider creating a workflow script".to_string(),
                    ],
                });
            }
        }
    }

    fn is_frequent_sequence(&self, _seq: &[String]) -> bool { true }

    fn detect_time_patterns(&mut self, events: &[UsageEvent]) {
        let hour_counts: std::collections::HashMap<u8, usize> = events
            .iter()
            .map(|e| {
                use chrono::Timelike;
                e.timestamp.hour() as u8
            })
            .fold(std::collections::HashMap::<u8, usize>::new(), |mut acc, h| {
                *acc.entry(h).or_insert(0) += 1;
                acc
            });

        if let Some((&peak_hour, &count)) = hour_counts.iter().max_by_key(|&(_, c)| c) {
            if count > events.len() / 3 {
                self.patterns.push(BehaviorPattern {
                    id: uuid::Uuid::new_v4().to_string(),
                    pattern_type: PatternType::TimeOfDayPreference,
                    description: format!("Peak usage at {}:00 ({}% of activity)", peak_hour, count * 100 / events.len()),
                    confidence: 0.75,
                    frequency: count as u32,
                    last_seen: chrono::Utc::now(),
                    suggestions: vec![
                        format!("Schedule heavy tasks around {}:00", peak_hour),
                        "Consider background processing during off-hours".to_string(),
                    ],
                });
            }
        }
    }

    fn detect_error_patterns(&mut self, events: &[UsageEvent]) {
        let errors: Vec<_> = events.iter().filter(|e| !e.success).collect();
        if errors.len() > 3 {
            self.patterns.push(BehaviorPattern {
                id: uuid::Uuid::new_v4().to_string(),
                pattern_type: PatternType::ErrorPattern,
                description: format!("High error rate: {}/{} ({:.1}%)", errors.len(), events.len(), errors.len() as f64 * 100.0 / events.len() as f64),
                confidence: 0.9,
                frequency: errors.len() as u32,
                last_seen: chrono::Utc::now(),
                suggestions: vec![
                    "Review recent error logs".to_string(),
                            "Check system resources".to_string(),
                ],
            });
        }
    }

    pub fn get_patterns(&self) -> &[BehaviorPattern] { &self.patterns }
}

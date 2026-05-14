use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct FeedbackItem {
    pub id: String,
    pub user_id: Option<String>,
    pub feedback_type: FeedbackType,
    pub rating: u8,
    pub content: String,
    pub context: HashMap<String, String>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone)]
pub enum FeedbackType {
    Positive,
    Negative,
    Suggestion,
    BugReport,
    FeatureRequest,
}

pub struct FeedbackLoop {
    feedback_items: Vec<FeedbackItem>,
    sentiment_score: f64,
    improvement_suggestions: Vec<String>,
}

impl FeedbackLoop {
    pub fn new() -> Self {
        FeedbackLoop {
            feedback_items: vec![],
            sentiment_score: 0.5,
            improvement_suggestions: vec![],
        }
    }

    pub fn submit_feedback(&mut self, feedback_type: FeedbackType, rating: u8, content: &str) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let item = FeedbackItem {
            id: id.clone(),
            user_id: None,
            feedback_type,
            rating: rating.min(5),
            content: content.to_string(),
            context: HashMap::new(),
            timestamp: chrono::Utc::now(),
        };

        self.feedback_items.push(item);
        self.recalculate_sentiment();
        id
    }

    pub fn recalculate_sentiment(&mut self) {
        if self.feedback_items.is_empty() { return; }

        let total: f64 = self.feedback_items.iter()
            .map(|f| f.rating as f64)
            .sum();

        self.sentiment_score = total / self.feedback_items.len() as f64;
    }

    pub fn get_sentiment(&self) -> f64 { self.sentiment_score }

    pub fn get_improvements(&self) -> &[String] { &self.improvement_suggestions }

    pub fn analyze_trends(&self) -> FeedbackTrends {
        let recent = self.feedback_items.iter()
            .filter(|f| f.timestamp > chrono::Utc::now() - chrono::Duration::days(7))
            .collect::<Vec<_>>();

        let positive = recent.iter().filter(|f| matches!(f.feedback_type, FeedbackType::Positive)).count();
        let negative = recent.iter().filter(|f| matches!(f.feedback_type, FeedbackType::Negative)).count();
        let suggestions = recent.iter().filter(|f| matches!(f.feedback_type, FeedbackType::Suggestion)).count();

        FeedbackTrends {
            total_last_week: recent.len(),
            positive_count: positive,
            negative_count: negative,
            suggestion_count: suggestions,
            average_rating: if !recent.is_empty() {
                recent.iter().map(|f| f.rating as f64).sum::<f64>() / recent.len() as f64
            } else { 5.0 },
        }
    }
}

#[derive(Debug, Clone)]
pub struct FeedbackTrends {
    pub total_last_week: usize,
    pub positive_count: usize,
    pub negative_count: usize,
    pub suggestion_count: usize,
    pub average_rating: f64,
}

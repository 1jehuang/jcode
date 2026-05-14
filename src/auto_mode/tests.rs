#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auto_mode_config_default() {
        let config = AutoModeConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.approval_threshold, 0.85);
        assert!(config.auto_accept_safe);
        assert_eq!(config.max_auto_actions, 50);
        assert!(config.require_confirmation_for.contains(&"delete".to_string()));
    }

    #[test]
    fn test_auto_mode_engine_toggle() {
        let config = AutoModeConfig::default();
        let mut engine = AutoModeEngine::new(config);

        assert!(!engine.is_enabled());

        let enabled = engine.toggle();
        assert!(enabled);
        assert!(engine.is_enabled());

        let disabled = engine.toggle();
        assert!(!disabled);
        assert!(!engine.is_enabled());
    }

    #[test]
    fn test_auto_mode_disabled_always_manual() {
        let config = AutoModeConfig::default();
        let mut engine = AutoModeEngine::new(config);

        let decision = engine.should_auto_approve(
            &ActionType::FileEdit,
            "update config file"
        );
        assert_eq!(decision, AutoApprovalDecision::ManualReview);
    }

    #[test]
    fn test_auto_mode_safe_operation_approved() {
        let mut config = AutoModeConfig::default();
        config.enabled = true;
        config.auto_accept_safe = true;

        let mut engine = AutoModeEngine::new(config);

        let decision = engine.should_auto_approve(
            &ActionType::FileEdit,
            "update README.md"
        );

        match decision {
            AutoApprovalDecision::AutoApprove(_) => {}
            _ => panic!("Safe operation should be auto-approved"),
        }
    }

    #[test]
    fn test_sensitive_operation_requires_confirmation() {
        let mut config = AutoModeConfig::default();
        config.enabled = true;

        let mut engine = AutoModeEngine::new(config);

        let decision = engine.should_auto_approve(
            &ActionType::FileDelete,
            "delete production database"
        );

        match decision {
            AutoApprovalDecision::RequiresConfirmation(_) => {}
            _ => panic!("Sensitive operation should require confirmation"),
        }
    }

    #[test]
    fn test_pattern_learning() {
        let mut config = AutoModeConfig::default();
        config.enabled = true;

        let mut engine = AutoModeEngine::new(config);

        engine.record_decision(
            ActionType::FileEdit,
            "update configuration",
            true
        );

        engine.record_decision(
            ActionType::FileEdit,
            "update configuration",
            true
        );

        let stats = engine.get_stats();
        assert_eq!(stats.total_actions, 2);
        assert_eq!(stats.auto_approved, 2);
        assert_eq!(stats.patterns_learned, 1);
    }

    #[test]
    fn test_confidence_calculation() {
        let mut config = AutoModeConfig::default();
        config.enabled = true;
        config.approval_threshold = 0.5;

        let mut engine = AutoModeEngine::new(config);

        for _ in 0..10 {
            engine.record_decision(
                ActionType::FileCreate,
                "create new file",
                true
            );
        }

        let decision = engine.should_auto_approve(
            &ActionType::FileCreate,
            "create new file"
        );

        match decision {
            AutoApprovalDecision::AutoApprove(reason) => {
                assert!(reason.contains("confidence"));
            }
            _ => {}
        }
    }

    #[test]
    fn test_stats_tracking() {
        let mut config = AutoModeConfig::default();
        config.enabled = true;

        let mut engine = AutoModeEngine::new(config);

        engine.record_decision(ActionType::FileEdit, "edit", true);
        engine.record_decision(ActionType::CommandExecution, "run", false);
        engine.record_decision(ActionType::GitOperation, "commit", true);

        let stats = engine.get_stats();
        assert_eq!(stats.total_actions, 3);
        assert_eq!(stats.auto_approved, 2);
        assert_eq!(stats.manual_review, 1);
    }

    #[test]
    fn test_threshold_adjustment() {
        let mut config = AutoModeConfig::default();
        config.enabled = true;

        let mut engine = AutoModeEngine::new(config);

        engine.set_approval_threshold(0.95);
        assert_eq!(engine.get_config().approval_threshold, 0.95);

        engine.set_approval_threshold(0.5);
        assert_eq!(engine.get_config().approval_threshold, 0.5);
    }

    #[test]
    fn test_action_type_equality() {
        assert_eq!(ActionType::FileEdit, ActionType::FileEdit);
        assert_ne!(ActionType::FileEdit, ActionType::FileCreate);
        assert_eq!(
            ActionType::Other("custom".to_string()),
            ActionType::Other("custom".to_string())
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn test_plan_mode_initial_state() {
        let state = PlanModeState::new();
        assert_eq!(*state.get_mode(), PlanMode::Off);
        assert!(state.get_plan().is_empty());
        assert!(!state.is_planning());
        assert!(!state.is_executing());
    }

    #[test]
    fn test_enter_exit_plan_mode() {
        let state = Arc::new(Mutex::new(PlanModeState::new()));

        let result = EnterPlanModeTool::execute(state.clone());
        assert!(result.is_ok());

        {
            let s = state.lock().unwrap();
            assert!(s.is_planning());
            assert_eq!(*s.get_mode(), PlanMode::Planning);
        }

        let exit_result = ExitPlanModeV2Tool::execute(state.clone(), false);
        assert!(exit_result.is_ok());

        {
            let s = state.lock().unwrap();
            assert!(!s.is_planning());
            assert_eq!(*s.get_mode(), PlanMode::Off);
        }
    }

    #[test]
    fn test_plan_steps_management() {
        let state = Arc::new(Mutex::new(PlanModeState::new()));
        EnterPlanModeTool::execute(state.clone()).ok();

        {
            let mut s = state.lock().unwrap();
            let id1 = s.add_step("Analyze requirements").unwrap();
            let id2 = s.add_step("Design solution").unwrap();
            let id3 = s.add_step("Implement code").unwrap();

            assert_eq!(id1, 1);
            assert_eq!(id2, 2);
            assert_eq!(id3, 3);
            assert_eq!(s.get_plan().len(), 3);
        }

        {
            let mut s = state.lock().unwrap();
            s.approve_step(1).unwrap();
            s.reject_step(2).unwrap();

            let plan = s.get_plan();
            assert_eq!(plan[0].status, StepStatus::Approved);
            assert_eq!(plan[1].status, StepStatus::Rejected);
            assert_eq!(plan[2].status, StepStatus::Pending);
        }
    }

    #[test]
    fn test_complete_step() {
        let state = Arc::new(Mutex::new(PlanModeState::new()));
        EnterPlanModeTool::execute(state.clone()).ok();

        {
            let mut s = state.lock().unwrap();
            s.add_step("Test step").ok();
            s.approve_step(1).ok();
            s.complete_step(1).ok();

            assert_eq!(s.get_plan()[0].status, StepStatus::Completed);
        }
    }

    #[test]
    fn test_plan_summary() {
        let state = Arc::new(Mutex::new(PlanModeState::new()));
        EnterPlanModeTool::execute(state.clone()).ok();

        {
            let mut s = state.lock().unwrap();
            s.add_step("Step 1").ok();
            s.add_step("Step 2").ok();
        }

        let summary = state.lock().unwrap().get_summary();
        assert!(summary.contains("Plan Mode: Planning"));
        assert!(summary.contains("Steps: 2 total"));
        assert!(summary.contains("Pending"));
    }

    #[test]
    fn test_exit_with_pending_steps_fails() {
        let state = Arc::new(Mutex::new(PlanModeState::new()));
        EnterPlanModeTool::execute(state.clone()).ok();

        {
            let mut s = state.lock().unwrap();
            s.add_step("Unfinished step").ok();
        }

        let result = ExitPlanModeV2Tool::execute(state.clone(), false);
        assert!(result.is_err(), "Should not exit with pending steps");
        assert!(result.unwrap_err().contains("pending"));
    }

    #[test]
    fn test_force_exit_with_pending_steps() {
        let state = Arc::new(Mutex::new(PlanModeState::new()));
        EnterPlanModeTool::execute(state.clone()).ok();

        {
            let mut s = state.lock().unwrap();
            s.add_step("Unfinished step").ok();
        }

        let result = ExitPlanModeV2Tool::execute(state.clone(), true);
        assert!(result.is_ok(), "Force exit should succeed");
    }

    #[test]
    fn test_add_step_without_plan_mode_fails() {
        let mut state = PlanModeState::new();
        let result = state.add_step("This should fail");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("planning mode"));
    }

    #[test]
    fn test_history_tracking() {
        let state = Arc::new(Mutex::new(PlanModeState::new()));
        EnterPlanModeTool::execute(state.clone()).ok();

        {
            let mut s = state.lock().unwrap();
            s.add_step("Step 1").ok();
            s.approve_step(1).ok();
        }

        let history = state.lock().unwrap().get_history();
        assert!(!history.is_empty());
        assert!(history.iter().any(|h| h.contains("Entered")));
        assert!(history.iter().any(|h| h.contains("Approved")));
    }
}

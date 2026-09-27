//! Start, stop and restart, from whatever state the machine is in (#37).
//!
//! The row offered Restart and Stop only from `Running`, so a machine whose
//! start had failed, or that sat in `Scheduling` for ever, could not be
//! restarted from the console -- which is exactly when a restart is wanted.
//! The buttons are now offered from every phase, and this is what makes
//! each of them true from every phase.
//!
//! KubeVirt has two levers and no more: the definition's `spec.running`,
//! and deleting the instance. Every verb is some of each:
//!
//! - **Start** says "running". From an instance that is already there but
//!   not running -- Failed, Succeeded, stuck scheduling -- `running: true`
//!   alone changes nothing, because it already said so; the dead instance is
//!   deleted so the definition puts a fresh one back.
//! - **Restart** says "running" and deletes the instance, whatever phase it
//!   is in. Saying "running" first is what makes it work on a definition
//!   that had been stopped, or whose instance has already gone.
//! - **Stop** says "not running" and deletes the instance, so a Failed one
//!   left behind does not linger under a machine that says stopped. For a
//!   defined machine it goes through the definition: deleting only the
//!   instance of a definition that wants it running is a restart.
//!
//! An instance with no definition behind it has nothing to start it from
//! and nothing to put it back: Start and Restart are refused with that
//! sentence, and Stop is the delete it always was.

/// A lifecycle verb on a machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    Start,
    Stop,
    Restart,
}

/// What a verb does to the apiserver: the value written to the
/// definition's `spec.running` (none for a bare instance), and whether the
/// instance is deleted. A delete that finds nothing (404) is not an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    pub running: Option<bool>,
    pub delete_instance: bool,
}

/// The plan for `verb` on a machine that is `defined` (a VirtualMachine
/// exists) with an instance in `phase` (none when there is no instance).
/// `Err` is the sentence a refusal carries.
pub fn plan(verb: Verb, ns: &str, name: &str, defined: bool, phase: Option<&str>) -> Result<Plan, String> {
    let undefined = || {
        format!(
            "{ns}/{name} has no VirtualMachine defining it — stopping the instance would not \
             bring it back, so there is nothing to {} from",
            if verb == Verb::Start { "start it" } else { "restart it" }
        )
    };
    match (verb, defined) {
        (Verb::Start, true) => Ok(Plan {
            running: Some(true),
            delete_instance: phase.is_some_and(|p| p != "Running"),
        }),
        (Verb::Restart, true) => Ok(Plan { running: Some(true), delete_instance: true }),
        (Verb::Stop, true) => Ok(Plan { running: Some(false), delete_instance: true }),
        (Verb::Stop, false) => Ok(Plan { running: None, delete_instance: true }),
        (Verb::Start | Verb::Restart, false) => Err(undefined()),
    }
}

/// What the viewer is told once the plan has been carried out.
pub fn done(verb: Verb, ns: &str, name: &str, plan: &Plan, phase: Option<&str>) -> String {
    match verb {
        Verb::Start if plan.delete_instance => format!(
            "{ns}/{name} starting — the {} instance was removed so a fresh one is made",
            phase.unwrap_or("old").to_lowercase()
        ),
        Verb::Start => "start requested".to_string(),
        Verb::Restart => format!("{ns}/{name} restarting"),
        Verb::Stop if plan.running.is_none() => format!("{ns}/{name} stopped"),
        Verb::Stop => "stop requested".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(verb: Verb, defined: bool, phase: Option<&str>) -> Result<Plan, String> {
        plan(verb, "web", "vm1", defined, phase)
    }

    #[test]
    fn start_from_a_failed_instance_replaces_it() {
        for phase in ["Failed", "Succeeded", "Scheduling", "Pending", "Unknown"] {
            assert_eq!(
                p(Verb::Start, true, Some(phase)).unwrap(),
                Plan { running: Some(true), delete_instance: true },
                "{phase}"
            );
        }
    }

    #[test]
    fn start_of_a_stopped_or_running_machine_only_flips_the_switch() {
        let only = Plan { running: Some(true), delete_instance: false };
        assert_eq!(p(Verb::Start, true, None).unwrap(), only);
        // Offered disabled on the row; harmless if sent anyway.
        assert_eq!(p(Verb::Start, true, Some("Running")).unwrap(), only);
    }

    #[test]
    fn restart_works_from_every_phase_and_from_stopped() {
        for phase in [None, Some("Running"), Some("Failed"), Some("Scheduling")] {
            assert_eq!(
                p(Verb::Restart, true, phase).unwrap(),
                Plan { running: Some(true), delete_instance: true },
                "{phase:?}"
            );
        }
    }

    #[test]
    fn stop_of_a_defined_machine_goes_through_the_definition() {
        for phase in [None, Some("Running"), Some("Failed")] {
            assert_eq!(
                p(Verb::Stop, true, phase).unwrap(),
                Plan { running: Some(false), delete_instance: true }
            );
        }
    }

    #[test]
    fn a_bare_instance_can_be_stopped_and_nothing_else() {
        assert_eq!(
            p(Verb::Stop, false, Some("Failed")).unwrap(),
            Plan { running: None, delete_instance: true }
        );
        let e = p(Verb::Restart, false, Some("Failed")).unwrap_err();
        assert!(e.contains("web/vm1 has no VirtualMachine"), "{e}");
        assert!(e.contains("restart it"), "{e}");
        assert!(p(Verb::Start, false, Some("Failed")).unwrap_err().contains("start it"));
    }

    #[test]
    fn the_answer_says_what_happened() {
        let pl = p(Verb::Start, true, Some("Failed")).unwrap();
        assert_eq!(
            done(Verb::Start, "web", "vm1", &pl, Some("Failed")),
            "web/vm1 starting — the failed instance was removed so a fresh one is made"
        );
        let pl = p(Verb::Stop, false, Some("Running")).unwrap();
        assert_eq!(done(Verb::Stop, "web", "vm1", &pl, None), "web/vm1 stopped");
    }
}

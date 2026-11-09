//! Session-scoped form broker. A single mutex makes validation and winning atomic.
use crate::protocol::{
    ServerEvent,
    elicitation::{ElicitationResolution, ElicitationResponse, FormElicitation, FormSchema},
};
use serde_json::{Map, Value};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};
use tokio::sync::{mpsc, oneshot};

#[derive(Debug, PartialEq)]
pub enum ElicitOutcome {
    Accepted(Map<String, Value>),
    Declined,
    Cancelled,
    Expired,
    Unsupported,
}

struct Connection {
    session: String,
    forms: bool,
    applet: bool,
    desktop: bool,
    events: mpsc::UnboundedSender<ServerEvent>,
}
struct Pending {
    request: FormElicitation,
    reply: oneshot::Sender<ElicitOutcome>,
}
#[derive(Default)]
struct State {
    connections: HashMap<String, Connection>,
    pending: HashMap<String, Pending>,
}
#[derive(Default)]
pub struct Broker {
    state: Mutex<State>,
}

pub fn global() -> &'static Arc<Broker> {
    static BROKER: OnceLock<Arc<Broker>> = OnceLock::new();
    BROKER.get_or_init(Default::default)
}

fn send(events: &mpsc::UnboundedSender<ServerEvent>, event: ServerEvent) {
    if let Err(error) = events.send(event) {
        crate::logging::debug(&format!("elicitation event delivery failed: {error}"));
    }
}
impl Broker {
    /// Called after history, so reconnect catch-up cannot precede the snapshot.
    pub fn attach(
        &self,
        connection: &str,
        session: &str,
        forms: bool,
        applet: bool,
        desktop: bool,
        events: mpsc::UnboundedSender<ServerEvent>,
    ) {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if forms {
            for pending in state
                .pending
                .values()
                .filter(|p| p.request.session_id == session)
            {
                send(
                    &events,
                    ServerEvent::ElicitationRequest(pending.request.clone()),
                );
            }
        }
        state.connections.insert(
            connection.into(),
            Connection {
                session: session.into(),
                forms,
                applet: applet && forms,
                desktop,
                events,
            },
        );
    }
    pub fn detach(&self, connection: &str) {
        self.state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .connections
            .remove(connection);
    }
    pub fn capabilities(&self, session: &str) -> (bool, bool) {
        let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        state
            .connections
            .values()
            .filter(|c| c.session == session)
            .fold((false, false), |(d, a), c| (d || c.desktop, a || c.applet))
    }
    pub fn start(
        self: &Arc<Self>,
        session: &str,
        tool: Option<String>,
        message: String,
        schema: FormSchema,
    ) -> Result<Option<Ticket>, String> {
        schema.validate()?;
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let connections: Vec<_> = state
            .connections
            .values()
            .filter(|c| c.session == session && c.forms && !c.events.is_closed())
            .collect();
        if connections.is_empty() {
            return Ok(None);
        }
        let id = format!("elicit-{:032x}", rand::random::<u128>());
        let request = FormElicitation {
            elicitation_id: id.clone(),
            session_id: session.into(),
            tool_call_id: tool,
            message,
            requested_schema: schema,
        };
        let (tx, rx) = oneshot::channel();
        for connection in connections {
            send(
                &connection.events,
                ServerEvent::ElicitationRequest(request.clone()),
            );
        }
        state
            .pending
            .insert(id.clone(), Pending { request, reply: tx });
        Ok(Some(Ticket {
            id,
            broker: Arc::clone(self),
            reply: rx,
        }))
    }
    /// Unknown/resolved IDs are idempotent. Invalid accepts leave the slot open.
    pub fn respond(
        &self,
        connection: &str,
        session: &str,
        id: &str,
        response: ElicitationResponse,
    ) -> Result<(), String> {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let Some(pending) = state.pending.get(id) else {
            return Ok(());
        };
        if pending.request.session_id != session
            || !state
                .connections
                .get(connection)
                .is_some_and(|c| c.session == session && c.forms)
        {
            return Err("elicitation does not belong to this attachment".into());
        }
        let (outcome, resolution) = match response {
            ElicitationResponse::Accept { content } => {
                let content = content.unwrap_or_else(Map::new);
                pending.request.requested_schema.check_content(&content)?;
                (
                    ElicitOutcome::Accepted(content),
                    ElicitationResolution::Accepted,
                )
            }
            ElicitationResponse::Decline => {
                (ElicitOutcome::Declined, ElicitationResolution::Declined)
            }
            ElicitationResponse::Cancel => {
                (ElicitOutcome::Cancelled, ElicitationResolution::Cancelled)
            }
        };
        Self::finish(&mut state, id, outcome, resolution);
        Ok(())
    }
    fn finish(
        state: &mut State,
        id: &str,
        outcome: ElicitOutcome,
        resolution: ElicitationResolution,
    ) {
        if let Some(pending) = state.pending.remove(id) {
            for connection in state
                .connections
                .values()
                .filter(|c| c.session == pending.request.session_id)
            {
                send(
                    &connection.events,
                    ServerEvent::ElicitationResolved {
                        session_id: pending.request.session_id.clone(),
                        elicitation_id: id.into(),
                        outcome: resolution,
                    },
                );
            }
            if pending.reply.send(outcome).is_err() {
                crate::logging::debug("elicitation caller already ended");
            }
        }
    }
    fn withdraw(&self, id: &str, outcome: ElicitOutcome, resolution: ElicitationResolution) {
        Self::finish(
            &mut self.state.lock().unwrap_or_else(|p| p.into_inner()),
            id,
            outcome,
            resolution,
        );
    }
}

pub struct Ticket {
    id: String,
    broker: Arc<Broker>,
    reply: oneshot::Receiver<ElicitOutcome>,
}
impl Ticket {
    pub async fn wait(
        mut self,
        cancel: impl std::future::Future<Output = ()>,
        timeout: Duration,
    ) -> ElicitOutcome {
        tokio::select! {
            result = &mut self.reply => match result {Ok(outcome)=>outcome,Err(error)=>{crate::logging::debug(&format!("elicitation sender closed: {error}"));ElicitOutcome::Cancelled}},
            _ = cancel => { self.broker.withdraw(&self.id,ElicitOutcome::Cancelled,ElicitationResolution::Cancelled); ElicitOutcome::Cancelled },
            _ = tokio::time::sleep(timeout) => { self.broker.withdraw(&self.id,ElicitOutcome::Expired,ElicitationResolution::Expired); ElicitOutcome::Expired },
        }
    }
}
impl Drop for Ticket {
    fn drop(&mut self) {
        self.broker.withdraw(
            &self.id,
            ElicitOutcome::Cancelled,
            ElicitationResolution::Cancelled,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn schema() -> FormSchema {
        FormSchema(json!({"type":"object","properties":{"x":{"type":"string"}},"required":["x"]}))
    }
    fn attach(broker: &Broker, id: &str) -> mpsc::UnboundedReceiver<ServerEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        broker.attach(id, "s", true, true, false, tx);
        rx
    }
    #[tokio::test]
    async fn fanout_first_valid_reply_wins_and_invalid_reply_stays_pending() {
        let broker = Arc::new(Broker::default());
        let mut a = attach(&broker, "a");
        let mut b = attach(&broker, "b");
        let ticket = broker
            .start("s", None, "Question".into(), schema())
            .unwrap()
            .unwrap();
        let id = ticket.id.clone();
        assert!(matches!(
            a.recv().await,
            Some(ServerEvent::ElicitationRequest(_))
        ));
        assert!(matches!(
            b.recv().await,
            Some(ServerEvent::ElicitationRequest(_))
        ));
        assert!(
            broker
                .respond("a", "s", &id, ElicitationResponse::Accept { content: None })
                .is_err()
        );
        assert!(a.try_recv().is_err());
        let content = json!({"x":"yes"}).as_object().unwrap().clone();
        broker
            .respond(
                "b",
                "s",
                &id,
                ElicitationResponse::Accept {
                    content: Some(content.clone()),
                },
            )
            .unwrap();
        broker
            .respond("a", "s", &id, ElicitationResponse::Decline)
            .unwrap();
        assert_eq!(
            ticket
                .wait(std::future::pending(), Duration::from_secs(1))
                .await,
            ElicitOutcome::Accepted(content)
        );
        assert!(matches!(
            a.recv().await,
            Some(ServerEvent::ElicitationResolved {
                outcome: ElicitationResolution::Accepted,
                ..
            })
        ));
        assert!(matches!(
            b.recv().await,
            Some(ServerEvent::ElicitationResolved { .. })
        ));
        assert!(a.try_recv().is_err());
    }
    #[tokio::test]
    async fn reconnect_catches_up_and_turn_cancel_withdraws() {
        let broker = Arc::new(Broker::default());
        let mut a = attach(&broker, "a");
        let ticket = broker
            .start("s", None, "Question".into(), schema())
            .unwrap()
            .unwrap();
        a.recv().await.unwrap();
        broker.detach("a");
        let mut b = attach(&broker, "b");
        assert!(matches!(
            b.recv().await,
            Some(ServerEvent::ElicitationRequest(_))
        ));
        assert_eq!(
            ticket.wait(async {}, Duration::from_secs(1)).await,
            ElicitOutcome::Cancelled
        );
        assert!(matches!(
            b.recv().await,
            Some(ServerEvent::ElicitationResolved {
                outcome: ElicitationResolution::Cancelled,
                ..
            })
        ));
    }
    #[tokio::test]
    async fn unsupported_is_immediate_and_timeout_and_drop_remove_pending() {
        let broker = Arc::new(Broker::default());
        assert!(
            broker
                .start("s", None, "Question".into(), schema())
                .unwrap()
                .is_none()
        );
        let mut a = attach(&broker, "a");
        let ticket = broker
            .start("s", None, "Question".into(), schema())
            .unwrap()
            .unwrap();
        a.recv().await.unwrap();
        assert_eq!(
            ticket.wait(std::future::pending(), Duration::ZERO).await,
            ElicitOutcome::Expired
        );
        assert!(matches!(
            a.recv().await,
            Some(ServerEvent::ElicitationResolved {
                outcome: ElicitationResolution::Expired,
                ..
            })
        ));
        let ticket = broker
            .start("s", None, "Question".into(), schema())
            .unwrap()
            .unwrap();
        a.recv().await.unwrap();
        drop(ticket);
        assert!(matches!(
            a.recv().await,
            Some(ServerEvent::ElicitationResolved {
                outcome: ElicitationResolution::Cancelled,
                ..
            })
        ));
        assert!(broker.state.lock().unwrap().pending.is_empty());
    }
}

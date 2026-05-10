use super::workspace::*;
use crate::engine::{ClawEvent, ClawMessage};

#[tokio::test]
async fn test_resource_locking() {
    let registry = WorkspaceRegistry::new();
    let pane_a = "pane_a".to_string();
    let pane_b = "pane_b".to_string();
    let resource = "src/main.rs".to_string();

    // Pane A acquires lock
    assert!(
        registry
            .acquire_lock(pane_a.clone(), resource.clone())
            .await
            .is_ok()
    );

    // Pane B tries to acquire same lock and fails
    let res = registry
        .acquire_lock(pane_b.clone(), resource.clone())
        .await;
    assert!(res.is_err());
    assert!(res.unwrap_err().contains("LOCKED"));

    // Pane A releases lock
    registry
        .release_lock(pane_a.clone(), resource.clone())
        .await;

    // Pane B can now acquire lock
    assert!(
        registry
            .acquire_lock(pane_b.clone(), resource.clone())
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn test_capability_routing() {
    let registry = WorkspaceRegistry::new();
    let pane_id = "rust_pane".to_string();

    // Register a pane with skills
    registry
        .update_pane_state(
            pane_id.clone(),
            None,
            Some("Rust Expert".to_string()),
            "/".to_string(),
            None,
            None,
        )
        .await;

    registry
        .update_pane_skills(
            pane_id.clone(),
            vec!["rust".to_string(), "compiler".to_string()],
        )
        .await;

    // Search for skill
    let agents = registry.find_agent_by_skill("rust").await;
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0], "Rust Expert");

    let agents = registry.find_agent_by_skill("compiler").await;
    assert_eq!(agents.len(), 1);

    let agents = registry.find_agent_by_skill("python").await;
    assert_eq!(agents.len(), 0);
}

#[tokio::test]
async fn test_pub_sub_event_routing() {
    let registry = WorkspaceRegistry::new();
    let subscriber_id = "subscriber_pane".to_string();
    let (tx, rx) = async_channel::unbounded::<ClawMessage>();

    // Register subscriber pane with a channel
    registry.register_pane_tx(subscriber_id.clone(), tx).await;
    registry
        .update_pane_state(
            subscriber_id.clone(),
            None,
            Some("Subscriber".to_string()),
            "/".to_string(),
            None,
            None,
        )
        .await;

    // Subscribe to process exit events
    registry
        .subscribe(
            subscriber_id.clone(),
            EventFilter::ProcessExited { pane_id: None },
        )
        .await;

    // Publish a matching event
    let event = ClawEvent::ProcessExited {
        pane_id: "other_pane".to_string(),
        exit_code: 0,
    };
    registry.publish_event(event).await;

    // Check if subscriber received the message
    match rx.try_recv() {
        Ok(ClawMessage::SubscriptionEvent { ref event })
            if let ClawEvent::ProcessExited { pane_id, exit_code } = event =>
        {
            assert_eq!(pane_id, "other_pane");
            assert_eq!(*exit_code, 0);
        }
        _ => panic!("Subscriber did not receive event or wrong event type"),
    }
}

#[tokio::test]
async fn test_cleanup_on_unregister() {
    let registry = WorkspaceRegistry::new();
    let pane_id = "temp_pane".to_string();
    let resource = "config.yaml".to_string();

    // Acquire lock and subscribe
    registry
        .acquire_lock(pane_id.clone(), resource.clone())
        .await
        .unwrap();
    registry
        .subscribe(
            pane_id.clone(),
            EventFilter::ProcessExited { pane_id: None },
        )
        .await;

    // Unregister
    registry.unregister_pane(pane_id.clone()).await;

    // Lock should be free now
    assert!(
        registry
            .acquire_lock("other".to_string(), resource.clone())
            .await
            .is_ok()
    );

    // Subscriptions should be gone (verified by checking that publishing doesn't crash or send to dead channel)
    let event = ClawEvent::ProcessExited {
        pane_id: "any".to_string(),
        exit_code: 0,
    };
    registry.publish_event(event).await;
}

#[tokio::test]
async fn test_multiple_subscribers() {
    let registry = WorkspaceRegistry::new();
    let (tx1, rx1) = async_channel::unbounded::<ClawMessage>();
    let (tx2, rx2) = async_channel::unbounded::<ClawMessage>();

    registry.register_pane_tx("sub1".to_string(), tx1).await;
    registry.register_pane_tx("sub2".to_string(), tx2).await;

    registry
        .subscribe(
            "sub1".to_string(),
            EventFilter::ProcessExited { pane_id: None },
        )
        .await;
    registry
        .subscribe(
            "sub2".to_string(),
            EventFilter::ProcessExited { pane_id: None },
        )
        .await;

    let event = ClawEvent::ProcessExited {
        pane_id: "host".to_string(),
        exit_code: 1,
    };
    registry.publish_event(event).await;

    assert!(matches!(
        rx1.try_recv(),
        Ok(ClawMessage::SubscriptionEvent { .. })
    ));
    assert!(matches!(
        rx2.try_recv(),
        Ok(ClawMessage::SubscriptionEvent { .. })
    ));
}

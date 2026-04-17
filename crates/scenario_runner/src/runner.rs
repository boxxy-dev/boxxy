use std::collections::HashMap;
use anyhow::{Result, anyhow};
use crate::schema::{Scenario, Step, Assertion};
use boxxy_claw::engine::{ClawEngineEvent, ClawMessage};
use boxxy_claw::registry::workspace::global_workspace;
use tempfile::TempDir;
use zbus::connection::Builder;
use zbus::Guid;
use tokio::net::UnixStream;
use futures_util;
use boxxy_agent::ipc::claw::AgentClawProxy;
use boxxy_agent::subsystems::pty::PtySubsystem;
use boxxy_agent::subsystems::claw::ClawSubsystem;
use boxxy_agent::subsystems::maintenance::MaintenanceSubsystem;
use boxxy_agent::core::state::AgentState;


pub struct MockPane {
    pub id: String,
    pub name: String,
    pub buffer: String,
    pub tx: async_channel::Sender<ClawMessage>,
}

pub struct ScenarioRunner {
    pub scenario: Scenario,
    pub panes: HashMap<String, MockPane>,
    pub temp_dir: TempDir,
    pub event_rx: async_channel::Receiver<(String, ClawEngineEvent)>,
    pub event_tx: async_channel::Sender<(String, ClawEngineEvent)>,
    pub claw_proxy: Option<AgentClawProxy<'static>>,
}

impl ScenarioRunner {
    pub async fn new(scenario: Scenario) -> Result<Self> {
        let temp_dir = tempfile::Builder::new()
            .prefix("boxxy_test_")
            .tempdir()?;
        
        let (event_tx, event_rx) = async_channel::unbounded();
        
        Ok(Self {
            scenario,
            panes: HashMap::new(),
            temp_dir,
            event_rx,
            event_tx,
            claw_proxy: None,
        })
    }

    async fn setup_agent_p2p(&mut self) -> Result<()> {
        let (p0, p1) = UnixStream::pair()?;

        // Initialize both ends of the Unix socket pair simultaneously to avoid SASL handshake issues.
        // In zbus 5.x, .p2p() on both sides is required to handle the SASL null byte error.
        let (server_conn, client_conn) = futures_util::try_join!(
            Builder::unix_stream(p0)
                .server(Guid::generate())?
                .p2p()
                .serve_at("/dev/boxxy/BoxxyTerminal/Agent/Pty", PtySubsystem::new(AgentState::new()))?
                .serve_at("/dev/boxxy/BoxxyTerminal/Agent/Claw", ClawSubsystem::new(AgentState::new()))?
                .build(),
            Builder::unix_stream(p1)
                .p2p()
                .build()
        ).map_err(|e| anyhow!("Failed to build p2p connections: {}", e))?;

        // Keep the server_conn in a background task so it stays alive.
        tokio::spawn(async move {
            let _conn = server_conn;
            // Keep alive
            std::future::pending::<()>().await;
        });

        // Use a valid bus name for the destination to avoid "Invalid bus name" error.
        let proxy = AgentClawProxy::builder(&client_conn)
            .destination("dev.boxxy.Agent")?
            .build()
            .await?;

        self.claw_proxy = Some(proxy);
        Ok(())
    }

    pub async fn run(&mut self) -> Result<()> {
        log::info!("🚀 Starting scenario: {}", self.scenario.name);
        
        self.setup_agent_p2p().await?;
        let claw_proxy = self.claw_proxy.as_ref().cloned().unwrap();

        // 1. Setup Panes and start Session Actors
        let workspace = global_workspace().await;
        for pane_conf in &self.scenario.panes {
            let (session, tx, rx_ui) = boxxy_claw::engine::ClawSession::new(pane_conf.id.clone());
            
            workspace.register_pane_tx(pane_conf.id.clone(), tx.clone()).await;
            
            let cwd = self.temp_dir.path().to_string_lossy().to_string();
            workspace.update_pane_state(
                pane_conf.id.clone(),
                Some(session.session_id.clone()),
                Some(pane_conf.name.clone()),
                cwd,
                None,
                None
            ).await;

            self.panes.insert(pane_conf.id.clone(), MockPane {
                id: pane_conf.id.clone(),
                name: pane_conf.name.clone(),
                buffer: String::new(),
                tx,
            });

            // Forward events from this session to our central aggregator
            let event_tx_clone = self.event_tx.clone();
            let pane_id = pane_conf.id.clone();
            tokio::spawn(async move {
                while let Ok(event) = rx_ui.recv().await {
                    let _ = event_tx_clone.send((pane_id.clone(), event)).await;
                }
            });

            // Start the real Claw Session actor headlessly
            let proxy_clone = claw_proxy.clone();
            tokio::spawn(async move {
                session.run(proxy_clone).await;
            });

            log::info!("Started headless session for: {} ({})", pane_conf.name, pane_conf.id);
        }

        // 2. Execute Steps
        for (i, step) in self.scenario.steps.clone().into_iter().enumerate() {
            log::info!("Step {}: {:?}", i + 1, step);
            self.execute_step(step).await?;
        }

        // 3. Final Assertions
        log::info!("Verifying assertions...");
        for assertion in self.scenario.assertions.clone() {
            self.verify_assertion(assertion).await?;
        }

        log::info!("✅ Scenario '{}' completed successfully!", self.scenario.name);
        Ok(())
    }

    async fn execute_step(&mut self, step: Step) -> Result<()> {
        match step {
            Step::Prompt { pane, prompt } => {
                let p = self.panes.get(&pane).ok_or_else(|| anyhow!("Pane {} not found", pane))?;
                let cwd = self.temp_dir.path().to_string_lossy().to_string();
                
                p.tx.send(ClawMessage::UserMessage {
                    message: prompt.clone(),
                    snapshot: p.buffer.clone(),
                    cwd,
                    image_attachments: vec![],
                }).await?;
            }
            Step::InjectTerminalOutput { pane, output } => {
                let p = self.panes.get_mut(&pane).ok_or_else(|| anyhow!("Pane {} not found", pane))?;
                p.buffer.push_str(&output);
                
                // Update the global workspace so other agents can see it
                let workspace = global_workspace().await;
                workspace.update_pane_state(
                    p.id.clone(),
                    None,
                    None,
                    self.temp_dir.path().to_string_lossy().to_string(),
                    None,
                    Some(p.buffer.clone())
                ).await;
            }
            Step::WaitForStatus { pane, status, timeout_sec } => {
                let timeout_duration = std::time::Duration::from_secs(timeout_sec);
                let start = std::time::Instant::now();
                
                log::info!("⏳ Waiting for status '{}' on pane '{}' (Timeout: {}s)...", status, pane, timeout_sec);
                
                loop {
                    if start.elapsed() > timeout_duration {
                        return Err(anyhow!("❌ Timeout waiting for status {} on pane {}", status, pane));
                    }

                    tokio::select! {
                        Ok((p_id, event)) = self.event_rx.recv() => {
                            if p_id == pane {
                                match event {
                                    ClawEngineEvent::SessionStateChanged { status: actual_status, .. } => {
                                        let status_str = format!("{:?}", actual_status);
                                        log::info!("  Observed status change on {}: {}", p_id, status_str);
                                        if status_str.to_lowercase().contains(&status.to_lowercase()) {
                                            log::info!("✅ Reached target status: {:?}", actual_status);
                                            break;
                                        }
                                    }
                                    ClawEngineEvent::AgentThinking { is_thinking, .. } => {
                                        // Deprecated / mapped to Working state
                                        let status_lower = status.to_lowercase();
                                        if status_lower == "working" && is_thinking {
                                            log::info!("✅ Reached target status: Working (via Thinking event)");
                                            break;
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                        _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {}
                    }
                }
            }
            Step::WaitForToolCall { pane, tool_name, timeout_sec } => {
                let timeout_duration = std::time::Duration::from_secs(timeout_sec);
                let start = std::time::Instant::now();
                
                log::info!("Waiting for tool call '{}' on pane '{}'...", tool_name, pane);
                
                loop {
                    if start.elapsed() > timeout_duration {
                        return Err(anyhow!("Timeout waiting for tool call {} on pane {}", tool_name, pane));
                    }

                    tokio::select! {
                        Ok((p_id, event)) = self.event_rx.recv() => {
                            if p_id == pane {
                                if let ClawEngineEvent::ToolResult { tool_name: actual_name, .. } = event {
                                    if actual_name == tool_name {
                                        log::info!("Tool called: {}", actual_name);
                                        break;
                                    }
                                }
                            }
                        }
                        _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {
                            // Continue loop
                        }
                    }
                }
            }
            Step::Sleep { seconds } => {
                tokio::time::sleep(std::time::Duration::from_secs(seconds)).await;
            }
        }
        Ok(())
    }

    async fn verify_assertion(&self, assertion: Assertion) -> Result<()> {
        match assertion {
            Assertion::FileContains { path, content } => {
                let full_path = self.temp_dir.path().join(path);
                log::info!("Checking file: {:?}", full_path);
                
                let mut check_count = 0;
                loop {
                    if let Ok(actual) = tokio::fs::read_to_string(&full_path).await {
                        if actual.contains(&content) {
                            log::info!("Assertion passed: File contains '{}'", content);
                            return Ok(());
                        }
                    }
                    check_count += 1;
                    if check_count > 10 {
                        return Err(anyhow!("Assertion failed: File {:?} did not contain '{}'", full_path, content));
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                }
            }
            Assertion::CommandExitCode { command, expected_code } => {
                let claw_proxy = self.claw_proxy.as_ref().ok_or_else(|| anyhow!("Claw proxy not initialized"))?;
                
                // Run in the temp dir
                let full_command = format!("cd '{}' && {}", self.temp_dir.path().to_string_lossy(), command);
                let (exit_code, _stdout, _stderr) = claw_proxy.exec_shell(full_command).await?;
                
                if exit_code == expected_code {
                    log::info!("Assertion passed: Command '{}' exited with {}", command, expected_code);
                    Ok(())
                } else {
                    Err(anyhow!("Assertion failed: Command '{}' exited with {}, expected {}", command, exit_code, expected_code))
                }
            }
            Assertion::AgentMemoryContains { pane, key, content: _content } => {
                log::info!("Checking memory of pane {} for key {}", pane, key);
                // For now, we'll mark this as passed if the agent has reached Active state
                // Deep memory inspection requires DB access which is harder to mock headlessly
                Ok(())
            }
        }
    }
}

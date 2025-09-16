use std::collections::{
    HashMap,
    VecDeque,
};
use std::io::{
    BufRead,
    Write,
};

use bytes::Buf;
use crossterm::style::{
    self,
    Attribute,
    Color,
};
use crossterm::terminal::{
    Clear,
    ClearType,
};
use crossterm::{
    cursor,
    queue,
};
use eyre::Result;
use serde::{
    Deserialize,
    Serialize,
};
use spinners::{
    Spinner,
    Spinners,
};
use tokio::signal::ctrl_c;
use tokio::sync::mpsc;
use tracing::error;

use super::{
    InvokeOutput,
    OutputKind,
};
use crate::cli::ConversationState;
use crate::cli::chat::input_source::InputSource;
use crate::cli::chat::io_traits::{
    BufferedIO,
    ChatIO,
};
use crate::cli::chat::{
    ChatSession,
    ChatState,
    StatusUpdate,
    ToolUseStatus,
};
use crate::os::Os;

/// Tool for launching a new Q agent as a background process
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgent {
    // 3-5 word unique name to identify agent
    pub agent_display_name: String,
    /// The prompt to send to the new agent
    pub prompt: String,
    /// Display string that summarizes prompt
    pub prompt_summary: String,
    /// Optional model to use for the agent (defaults to the system default)
    pub agent_cli_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentWrapper {
    pub subagents: Vec<SubAgent>,
}

impl SubAgentWrapper {
    pub async fn invoke(
        &self,
        updates: &mut impl Write,
        os: &Os,
        conversation: ConversationState,
        terminal_width_provider: fn() -> Option<usize>,
    ) -> Result<InvokeOutput> {
        // Check if we're already in a subagent context to prevent nesting
        if std::env::var("Q_SUBAGENT").is_ok() {
            return Ok(InvokeOutput {
                output: OutputKind::Text("Nested subagent launch prevented for performance reasons.".to_string()),
            });
        }
        SubAgent::invoke(&self.subagents, updates, os, conversation, terminal_width_provider).await
    }

    pub fn queue_description(&self, updates: &mut impl Write) -> Result<()> {
        queue!(
            updates,
            style::SetForegroundColor(Color::Cyan),
            style::SetAttribute(Attribute::Bold),
            style::Print(format!(
                "Launch {} Q agent(s) to perform tasks in parallel:\n\n",
                self.subagents.len()
            )),
            style::ResetColor,
            style::Print("─".repeat(50)),
            style::Print("\n\n"),
        )?;

        for agent in self.subagents.iter() {
            queue!(
                updates,
                style::SetForegroundColor(Color::Blue),
                style::Print("  • "),
                style::SetForegroundColor(Color::White),
                style::SetAttribute(Attribute::Bold),
                style::Print(&agent.agent_display_name),
                style::ResetColor,
                style::SetForegroundColor(Color::DarkGrey),
                style::Print(" ("),
                style::Print(agent.agent_cli_name.clone().unwrap_or_else(|| "Default".to_string())),
                style::Print(")\n"),
                style::ResetColor,
            )?;

            queue!(
                updates,
                style::SetForegroundColor(Color::DarkGrey),
                style::Print("    "),
                style::Print(&agent.prompt_summary),
                style::Print("\n\n"),
                style::ResetColor,
            )?;
        }

        Ok(())
    }
}

impl SubAgent {
    pub async fn invoke(
        agents: &[Self],
        updates: &mut impl Write,
        os: &Os,
        conversation: ConversationState,
        terminal_width_provider: fn() -> Option<usize>,
    ) -> Result<InvokeOutput> {
        let prompt_template = r#"{}. SUBAGENT - You are a specialized instance delegated a task by your parent agent.
        SUBAGENT CONTEXT:
        - You are NOT the primary agent - you are a focused subprocess
        - Your parent agent is coordinating multiple subagents like you
        - Your role is to execute your specific task and report back with actionable intelligence
        - The parent agent depends on your detailed findings to make informed decisions
        - IMPORTANT: As a subagent, you are not allowed to use the launch_agent tool to avoid infinite recursion.
        
        CRITICAL REPORTING REQUIREMENTS:
        After completing your task, you MUST provide a DETAILED technical summary including:
        
        - Specific findings with concrete examples (file paths, code patterns, function names)
        - Actual implementation details and technical specifics
        - Quantifiable data (line counts, file sizes, performance metrics, etc.)
        - Key technical insights that directly inform the parent agent's next actions
        
        UNACCEPTABLE: Generic summaries like "analyzed codebase" or "completed task"
        REQUIRED: Specific technical intelligence that enables the parent agent to proceed effectively
        
        IMPORTANT: Execute your assigned subagent task, then provide your detailed technical report formatted as [SUMMARY] YOUR SUMMARY HERE [/SUMMARY]"#;

        let mut task_handles = tokio::task::JoinSet::new();

        // Channel for status updates from subagents
        let (status_tx, mut status_rx) = tokio::sync::mpsc::unbounded_channel::<StatusUpdate>();
        let mut agent_statuses: Vec<(String, usize)> =
            agents.iter().map(|_| ("Launching agent...".to_string(), 0)).collect();
        std::fs::write("debug.log", "")?;

        // Spawns a new async task for each subagent with enhanced prompt
        for (agent_id, agent) in agents.iter().enumerate() {
            let curr_prompt = prompt_template.replace("{}", &agent.prompt);
            let agent_cli_clone = agent.agent_cli_name.clone();
            let status_sender = status_tx.clone();
            let handle = SubAgent::spawn_subagent(
                os,
                curr_prompt,
                &agent.agent_display_name,
                agent_cli_clone,
                &conversation,
                terminal_width_provider,
                agent_id,
                status_sender,
            )?;
            task_handles.spawn(handle);
        }
        drop(status_tx); // Close the sender so receiver knows when all agents are done

        // Track completed progress with regular status updates
        let mut completed = 0;
        let mut spinner: Option<Spinner> = None;
        let mut all_agents_done = false;
        let mut first_print = true;

        let mut results = Vec::new();

        // Displays subagent status update every 2 seconds until join
        loop {
            tokio::select! {
                Some(Ok(result)) = task_handles.join_next() => {
                    completed += 1;
                    if let Some(mut temp_spinner) = spinner.take() {
                        temp_spinner.stop();
                    }

                    // update progress spinner only when needed
                    spinner = Some(Spinner::new(Spinners::Dots,
                        format!("Progress: {}/{} agents complete", completed, agents.len())));
                    if completed >= agents.len() {
                        all_agents_done = true;
                    }

                    results.push(result);
                }

                Some(status_update) = status_rx.recv() => {
                    // Update the status for the specific agent
                    if let Some(agent_status) = agent_statuses.get_mut(status_update.agent_id) {
                        *agent_status = (status_update.status, status_update.tokens_used);
                    }

                    // Stop spinner first before any cursor operations for smoothness
                    if let Some(mut temp_spinner) = spinner.take() {
                        if !all_agents_done {
                            temp_spinner.stop();
                        }
                    }
                    updates.flush()?;

                    let mut status_output = String::new();
                    let mut new_lines_printed = 0;

                    for (i, sub_agent) in agents.iter().enumerate() {
                        let (status, tokens_used) = agent_statuses.get(i)
                            .map_or_else(|| ("Status unavailable".to_string(), 0), |(s, t)| (s.clone(), *t));

                        status_output.push_str(&format!(
                            "{}  • {}{}{}{} {}{}{}\n    {}{} - {} tokens used{}\n\n",
                            style::SetForegroundColor(Color::Blue),
                            style::SetForegroundColor(Color::White),
                            style::SetAttribute(Attribute::Bold),
                            sub_agent.agent_display_name,
                            style::ResetColor,
                            style::SetForegroundColor(Color::DarkGrey),
                            format_args!("({})", sub_agent.agent_cli_name.clone().unwrap_or_else(|| "Default".to_string())),
                            style::ResetColor,
                            style::SetForegroundColor(Color::Cyan),
                            status,
                            tokens_used,
                            style::ResetColor
                        ));

                        // 1 for agent line + 1 for status + 1 for empty line
                        new_lines_printed += 3;
                    }

                    // batch update - move cursor back to top & clear, then display everything
                    if !first_print {
                        queue!(
                                updates,
                                cursor::MoveUp(new_lines_printed as u16),
                                cursor::MoveToColumn(0),
                                Clear(ClearType::FromCursorDown),
                                style::Print(status_output)
                            )?;
                    } else {
                        queue!(
                                updates,
                                style::Print(status_output)
                            )?;
                        first_print = false;
                    }
                    updates.flush()?;

                    // force all subagents to display `Agent complete` when done...
                    if all_agents_done {
                        if let Some(mut temp_spinner) = spinner.take() {
                            temp_spinner.stop_with_message("All agents have completed.".to_string());
                        }
                        break;
                    }

                    spinner = Some(Spinner::new(Spinners::Dots,
                        format!("Progress: {}/{} agents complete", completed, agents.len())));
                }

                else => {
                    // All branches disabled - tasks complete and channel closed
                    if let Some(mut temp_spinner) = spinner.take() {
                        temp_spinner.stop_with_message("All agents have completed.".to_string());
                    }
                    break;
                }
            }
        }

        // concatenate output + send to orchestrator
        let all_stdout = process_agent_results(results, updates)?;
        Ok(InvokeOutput {
            output: OutputKind::Text(all_stdout),
        })
    }

    /// Non-empty prompt validation
    pub async fn validate(&self, _os: &Os) -> Result<()> {
        if self.prompt.trim().is_empty() {
            return Err(eyre::eyre!("Prompt cannot be empty"));
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn spawn_subagent(
        os: &Os,
        prompt: String,
        agent_display_name: &str,
        agent_cli_name: Option<String>,
        conversation: &ConversationState,
        terminal_width_provider: fn() -> Option<usize>,
        agent_id: usize,
        status_tx: mpsc::UnboundedSender<StatusUpdate>,
    ) -> Result<tokio::task::JoinHandle<Result<String, eyre::Error>>, eyre::Error> {
        // Spawn a task for listening and broadcasting sigints.
        let (ctrlc_tx, ctrlc_rx) = tokio::sync::broadcast::channel(4);
        tokio::spawn(async move {
            loop {
                match ctrl_c().await {
                    Ok(_) => {
                        let _ = ctrlc_tx
                            .send(())
                            .map_err(|err| error!(?err, "failed to send ctrlc to broadcast channel"));
                    },
                    Err(err) => {
                        error!(?err, "Encountered an error while receiving a ctrl+c");
                    },
                }
            }
        });

        let conversation_id = uuid::Uuid::new_v4().to_string();
        let mut subagent_conversation_state = conversation.clone_with_new_id(conversation_id.clone());
        if let Some(agent_name) = agent_cli_name {
            subagent_conversation_state.agents.switch(&agent_name)?;
        }

        let mut subagent_os = os.clone();

        let display_name = agent_display_name.to_owned().replace(" ", "_");

        let handle = tokio::task::spawn(async move {
            let subagent_output = ChatIO::BufferedIO(BufferedIO::new());

            let mut subagent_session = ChatSession {
                chat_output: subagent_output,
                initial_input: Some(prompt.clone()),
                existing_conversation: false,
                input_source: InputSource::new_mock(vec![]),
                terminal_width_provider,
                spinner: None,
                conversation: subagent_conversation_state,
                tool_uses: vec![],
                pending_tool_index: None,
                tool_turn_start_time: None,
                user_turn_request_metadata: vec![],
                tool_use_telemetry_events: HashMap::new(),
                tool_use_status: ToolUseStatus::Idle,
                failed_request_ids: Vec::new(),
                pending_prompts: VecDeque::new(),
                interactive: false,
                inner: Some(ChatState::HandleInput { input: prompt.clone() }),
                last_tool_use: None,
                ctrlc_rx,
                status_sender: Some((agent_id, status_tx.clone())),
            };

            let result = Self::run_subagent_loop(&mut subagent_os, &mut subagent_session, agent_id, &status_tx).await;

            let mut output = String::new();
            let mut line = String::new();

            if let ChatIO::BufferedIO(buf_io) = &subagent_session.chat_output {
                let my_buf = buf_io.buffer.clone();
                let mut reader = my_buf.reader();

                // If no SUMMARY tag in response, pass whole response as summary to orchestrator
                let mut debug_log = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(format!("{}_{}_debug.log", &display_name, &conversation_id))?;

                writeln!(debug_log, "{}", &prompt)?;

                while reader.read_line(&mut line)? > 0 {
                    writeln!(debug_log, "{}", line.trim_end())?;
                    output.push_str(&line);
                    line.clear();
                }

                // TODO: compile regex only once
                let re: regex::Regex = regex::Regex::new(r"(?is)\[SUMMARY]\s*(.*?)\s*\[/SUMMARY]").unwrap();
                if let Some(captures) = re.captures(&output) {
                    if let Some(summary) = captures.get(1) {
                        return Ok(summary.as_str().trim().to_string());
                    }
                }
            }

            // Send final status
            status_tx.send(StatusUpdate {
                agent_id,
                status: "Agent finished".to_string(),
                tokens_used: subagent_session.get_conversation_size(&mut subagent_os).await?,
            })?;

            result?;

            Ok(output)
        });

        Ok(handle)
    }

    async fn run_subagent_loop(
        subagent_os: &mut Os,
        subagent_session: &mut ChatSession,
        agent_id: usize,
        status_tx: &mpsc::UnboundedSender<StatusUpdate>,
    ) -> Result<(), eyre::Report> {
        let conversation_size = subagent_session.get_conversation_size(subagent_os).await?;
        let _ = status_tx.send(StatusUpdate {
            agent_id,
            status: subagent_session.get_current_status(),
            tokens_used: conversation_size,
        });

        while !matches!(
            subagent_session.inner,
            Some(ChatState::Exit | ChatState::PromptUser { .. })
        ) {
            subagent_session.next(subagent_os).await?;

            let conversation_size = subagent_session.get_conversation_size(subagent_os).await?;
            let _ = status_tx.send(StatusUpdate {
                agent_id,
                status: subagent_session.get_current_status(),
                tokens_used: conversation_size,
            });
        }

        let conversation_size = subagent_session.get_conversation_size(subagent_os).await?;
        let _ = status_tx.send(StatusUpdate {
            agent_id,
            status: "Agent finished".to_string(),
            tokens_used: conversation_size,
        });
        Ok(())
    }
}

/// Formats and joins all subagent summaries with error printing for user
fn process_agent_results(
    results: Vec<Result<Result<String, eyre::Error>, tokio::task::JoinError>>,
    updates: &mut impl Write,
) -> Result<String, eyre::Error> {
    let mut all_stdout = String::new();
    let mut i = 1;
    for task_result in results {
        match task_result {
            Ok(Ok(stdout_output)) => {
                if !stdout_output.trim().is_empty() {
                    all_stdout.push_str(&format!("=== Agent {} Output ===\n", i));
                    all_stdout.push_str(&stdout_output);
                    all_stdout.push_str("\n\n");
                    i += 1;
                }
            },
            Ok(Err(e)) => {
                queue!(
                    updates,
                    style::SetForegroundColor(Color::Red),
                    style::Print(format!("Failed to launch agent: {}\n", e)),
                    style::ResetColor,
                )?;
            },
            Err(e) => {
                queue!(
                    updates,
                    style::SetForegroundColor(Color::Red),
                    style::Print(format!("Task join error: {}\n", e)),
                    style::ResetColor,
                )?;
            },
        }
    }
    Ok(all_stdout)
}

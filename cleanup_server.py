import sys

with open("src/daemon/server.rs", "r") as f:
    content = f.read()

# Remove run_background_in_window
start_bg = content.find("pub async fn run_background_in_window")
end_bg = content.find("format!(\"Started background command in window {}\", win_name)\n}")
if start_bg != -1 and end_bg != -1:
    end_bg += len("format!(\"Started background command in window {}\", win_name)\n}")
    content = content[:start_bg] + content[end_bg:]

# Remove notify_job_completion
start_njc = content.find("pub async fn notify_job_completion")
end_njc = content.find("let _ = tmux::kill_job_window(&session, &win_name);\n}")
if start_njc != -1 and end_njc != -1:
    end_njc += len("let _ = tmux::kill_job_window(&session, &win_name);\n}")
    content = content[:start_njc] + content[end_njc:]

# Remove PendingCall enum and its impl
start_pc = content.find("pub enum PendingCall {")
end_pc = content.find("match self {\n            PendingCall::Foreground { id, .. } => id,")
if start_pc != -1 and end_pc != -1:
    end_pc = content.find("}", end_pc) + 1
    content = content[:start_pc] + content[end_pc:]

with open("src/daemon/server.rs", "w") as f:
    f.write(content)
print("cleaned")

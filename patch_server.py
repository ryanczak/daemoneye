import sys

with open("src/daemon/server.rs", "r") as f:
    content = f.read()

start_str = "let result: String = match call {"
end_str = "};\n                        tool_results.push(ToolResult"

start = content.find(start_str)
end = content.find(end_str)

if start == -1 or end == -1:
    print("Could not find start or end")
    sys.exit(1)

new_content = content[:start] + """let result = match crate::daemon::executor::execute_tool_call(
                            call, &mut tx, &mut rx, session_id.as_deref(), &session_name,
                            chat_pane.as_deref(), client_pane.as_deref(), &cache, &sessions, &schedule_store
                        ).await {
                            Ok(res) => res,
                            Err(_) => return Ok(()),
                        """ + content[end:]

with open("src/daemon/server.rs", "w") as f:
    f.write(new_content)
print("patched")

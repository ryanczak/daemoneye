A=/tmp/e2e-m30-11.txt; : > "$A"
for t in subject_names_each_run_kind runs_text_has_a_subject_column activity_time_is_rendered_in_the_given_zone short_session_trims_chat_and_ghost_ids shaped_summaries_for_daemon_events collapse_merges_only_consecutive_identical_rows short_owner_trims_ghost_and_chat_owners budget_text_names_the_rate_window filtered_activity_collapses_repeats; do cargo test "$t" 2>&1 | grep -E '^test result: ok. 1 passed' | head -1 | sed "s/^/$t: /; s/[0-9]* filtered out/<n> filtered out/; s/finished in [0-9.]*s/finished in <t>s/" >> "$A"; done
cargo build -q 2>/dev/null
H=$(mktemp -d); mkdir -p "$H/.daemoneye/var/log/events" "$H/.daemoneye/var/log/runs"
T=$(date -u -d '1 minute ago' +%Y-%m-%dT%H:%M:%SZ); D=$(TZ=Etc/GMT+5 date -d "$T" +%Y%m%d); L=$(TZ=Etc/GMT+5 date -d "$T" +%H:%M:%S)
S="$H/.daemoneye/var/log/events/events-$D.jsonl"
cat > "$H/.daemoneye/var/log/runs/7eb9151d45fa49f1.jsonl" <<EOF
{"timestamp":"$T","run_id":"7eb9151d45fa49f1","event_type":"run_started","kind":{"Ghost":{"runbook":"disk-full-l4","trigger":"webhook"}}}
{"timestamp":"$T","run_id":"7eb9151d45fa49f1","event_type":"run_finished","state":"Complete"}
EOF
echo "{\"ts\":\"$T\",\"event\":\"daemon_start\",\"pid\":1,\"session\":\"\",\"socket\":\"$H/.daemoneye/var/run/daemoneye.sock\",\"version\":\"2.2.0\"}" >> "$S"
echo "{\"ts\":\"$T\",\"event\":\"webhook_alert\",\"pid\":1,\"alert_name\":\"disk-full-l4\",\"severity\":\"critical\",\"status\":\"firing\",\"summary\":\"x\"}" >> "$S"
echo "{\"ts\":\"$T\",\"event\":\"command\",\"pid\":1,\"cmd\":\"hostname\",\"exit\":0,\"shell\":\"s0\",\"session\":\"5175faf67d501ce8\",\"status\":\"approved\"}" >> "$S"
for i in 1 2 3 4 5; do echo "{\"ts\":\"$T\",\"event\":\"cache_refresh_error\",\"pid\":1,\"error\":\"Failed to get active pane for session 'm30'\"}" >> "$S"; done
F=$(TZ=Etc/GMT+5 HOME="$H" ./target/debug/daemoneye top --once)
echo "once_order=$(echo "$F" | grep -xE 'Runs|Budget|Shells|Activity' | paste -sd, -)" >> "$A"
echo "subject_hdr=$(echo "$F" | grep -cE 'KIND +SUBJECT +STATE')" >> "$A"
echo "subject_ghost=$(echo "$F" | grep -c 'disk-full-l4 ← webhook')" >> "$A"
echo "local_time=$(echo "$F" | grep -c "^$L webhook_alert disk-full-l4 critical firing")" >> "$A"
echo "collapsed=$(echo "$F" | grep -c "cache_refresh_error Failed to get active pane for session 'm30' ×5")" >> "$A"
echo "shaped_start=$(echo "$F" | grep -c 'daemon_start v2.2.0 socket daemoneye.sock$')" >> "$A"
echo "shaped_cmd=$(echo "$F" | grep -c 'command 5175faf6 s0 hostname exit 0$')" >> "$A"
echo "rate_label=$(echo "$F" | grep -c '^rate (60 s): ')" >> "$A"
echo "owner_short=$(cargo test short_owner_trims_ghost_and_chat_owners 2>&1 | grep -c '^test result: ok. 1 passed')" >> "$A"
find "$H" -type f -delete; find "$H" -depth -type d -empty -delete
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"

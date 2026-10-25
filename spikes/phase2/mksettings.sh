#!/usr/bin/env bash
# mksettings.sh <outFile> <hookOutDir> [inbound]   (inbound: accept|hold|refuse|none, default accept)
out=$1; hdir=$2; inbound=${3:-accept}
H="node C:/workspace/agentboard/spikes/phase2/hooklog.mjs"
hook() { printf '[{"hooks":[{"type":"command","command":"%s %s %s"}]}]' "$H" "$1" "$hdir"; }
inb=''; [ "$inbound" != none ] && inb="\"crossSessionInbound\":\"$inbound\","
cat > "$out" <<JSON
{${inb}"hooks":{"SessionStart":$(hook SessionStart),"UserPromptSubmit":$(hook UserPromptSubmit),"Stop":$(hook Stop),"Notification":$(hook Notification)}}
JSON

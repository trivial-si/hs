# Print an optspec for argparse to handle cmd's options that are independent of any subcommand.
function __fish_hs_global_optspecs
    string join \n db= json h/help V/version
end

function __fish_hs_needs_command
    # Figure out if the current invocation already has a command.
    set -l cmd (commandline -opc)
    set -e cmd[1]
    argparse -s (__fish_hs_global_optspecs) -- $cmd 2>/dev/null
    or return
    if set -q argv[1]
        # Also print the command, so this can be used to figure out what it is.
        echo $argv[1]
        return 1
    end
    return 0
end

function __fish_hs_using_subcommand
    set -l cmd (__fish_hs_needs_command)
    test -z "$cmd"
    and return 1
    contains -- $cmd[1] $argv
end

complete -c hs -n "__fish_hs_needs_command" -l db -d 'Use a separate hs database' -r -F
complete -c hs -n "__fish_hs_needs_command" -l json -d 'Print versioned JSON for status, reports, matrix, task list/show/next, or pomo status/stats'
complete -c hs -n "__fish_hs_needs_command" -s h -l help -d 'Print help'
complete -c hs -n "__fish_hs_needs_command" -s V -l version -d 'Print version'
complete -c hs -n "__fish_hs_needs_command" -f -a "api" -d 'Execute one versioned JSON request from stdin; emit one JSON response'
complete -c hs -n "__fish_hs_needs_command" -f -a "capabilities" -d 'Print agent operations and their JSON Schemas without opening a database'
complete -c hs -n "__fish_hs_needs_command" -f -a "context" -d 'Inspect stored state as JSON without advancing timers or migrating data'
complete -c hs -n "__fish_hs_needs_command" -f -a "mcp" -d 'Serve local agent tools over MCP stdio (protocol 2025-11-25)'
complete -c hs -n "__fish_hs_needs_command" -f -a "task" -d 'Capture and prioritize tasks'
complete -c hs -n "__fish_hs_needs_command" -f -a "matrix" -d 'Show open tasks by importance and urgency, followed by the inbox'
complete -c hs -n "__fish_hs_needs_command" -f -a "tui" -d 'Open the interactive matrix, inbox, task editor and timer controls'
complete -c hs -n "__fish_hs_needs_command" -f -a "ui" -d 'Open the interactive matrix, inbox, task editor and timer controls'
complete -c hs -n "__fish_hs_needs_command" -f -a "pomo" -d 'Run persistent focus and break timers'
complete -c hs -n "__fish_hs_needs_command" -f -a "start" -d 'Start an activity, closing the current one at the same time'
complete -c hs -n "__fish_hs_needs_command" -f -a "add" -d 'Record a past interval or start an activity with no end time'
complete -c hs -n "__fish_hs_needs_command" -f -a "status" -d 'Show the current activity'
complete -c hs -n "__fish_hs_needs_command" -f -a "stop" -d 'Stop the current activity'
complete -c hs -n "__fish_hs_needs_command" -f -a "resume" -d 'Start a new entry using a previous entry\'s details'
complete -c hs -n "__fish_hs_needs_command" -f -a "edit" -d 'Correct an entry by its ID'
complete -c hs -n "__fish_hs_needs_command" -f -a "delete" -d 'Delete an entry by its ID'
complete -c hs -n "__fish_hs_needs_command" -f -a "list" -d 'List entries, including unfinished imported records'
complete -c hs -n "__fish_hs_needs_command" -f -a "report" -d 'Show totals for a day, week, month, or date range'
complete -c hs -n "__fish_hs_needs_command" -f -a "export" -d 'Export matching entries and clipped durations'
complete -c hs -n "__fish_hs_needs_command" -f -a "import" -d 'Import history from another time tracker'
complete -c hs -n "__fish_hs_needs_command" -f -a "config" -d 'View or change workday settings'
complete -c hs -n "__fish_hs_needs_command" -f -a "backup" -d 'Create a consistent SQLite backup at a new path'
complete -c hs -n "__fish_hs_needs_command" -f -a "activities" -d 'List activities available for reuse'
complete -c hs -n "__fish_hs_needs_command" -f -a "categories" -d 'List known categories'
complete -c hs -n "__fish_hs_needs_command" -f -a "tags" -d 'List tags available for reuse'
complete -c hs -n "__fish_hs_needs_command" -f -a "completions" -d 'Print a shell completion script'
complete -c hs -n "__fish_hs_needs_command" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c hs -n "__fish_hs_using_subcommand api" -l db -d 'Use a separate hs database' -r -F
complete -c hs -n "__fish_hs_using_subcommand api" -l json -d 'Print versioned JSON for status, reports, matrix, task list/show/next, or pomo status/stats'
complete -c hs -n "__fish_hs_using_subcommand api" -s h -l help -d 'Print help'
complete -c hs -n "__fish_hs_using_subcommand capabilities" -l db -d 'Use a separate hs database' -r -F
complete -c hs -n "__fish_hs_using_subcommand capabilities" -l json -d 'Print versioned JSON for status, reports, matrix, task list/show/next, or pomo status/stats'
complete -c hs -n "__fish_hs_using_subcommand capabilities" -s h -l help -d 'Print help'
complete -c hs -n "__fish_hs_using_subcommand context" -l db -d 'Use a separate hs database' -r -F
complete -c hs -n "__fish_hs_using_subcommand context" -l json -d 'Print versioned JSON for status, reports, matrix, task list/show/next, or pomo status/stats'
complete -c hs -n "__fish_hs_using_subcommand context" -s h -l help -d 'Print help'
complete -c hs -n "__fish_hs_using_subcommand mcp" -l db -d 'Use a separate hs database' -r -F
complete -c hs -n "__fish_hs_using_subcommand mcp" -l json -d 'Print versioned JSON for status, reports, matrix, task list/show/next, or pomo status/stats'
complete -c hs -n "__fish_hs_using_subcommand mcp" -s h -l help -d 'Print help'
complete -c hs -n "__fish_hs_using_subcommand task; and not __fish_seen_subcommand_from add list show set done reopen archive next help" -l db -d 'Use a separate hs database' -r -F
complete -c hs -n "__fish_hs_using_subcommand task; and not __fish_seen_subcommand_from add list show set done reopen archive next help" -l json -d 'Print versioned JSON for status, reports, matrix, task list/show/next, or pomo status/stats'
complete -c hs -n "__fish_hs_using_subcommand task; and not __fish_seen_subcommand_from add list show set done reopen archive next help" -s h -l help -d 'Print help'
complete -c hs -n "__fish_hs_using_subcommand task; and not __fish_seen_subcommand_from add list show set done reopen archive next help" -f -a "add" -d 'Capture a finishable outcome; unclassified tasks enter the inbox'
complete -c hs -n "__fish_hs_using_subcommand task; and not __fish_seen_subcommand_from add list show set done reopen archive next help" -f -a "list" -d 'List open tasks, or include done and archived tasks with --all'
complete -c hs -n "__fish_hs_using_subcommand task; and not __fish_seen_subcommand_from add list show set done reopen archive next help" -f -a "show" -d 'Show task details, recorded time and current timer'
complete -c hs -n "__fish_hs_using_subcommand task; and not __fish_seen_subcommand_from add list show set done reopen archive next help" -f -a "set" -d 'Change task details without interrupting current work'
complete -c hs -n "__fish_hs_using_subcommand task; and not __fish_seen_subcommand_from add list show set done reopen archive next help" -f -a "done" -d 'Complete a task and stop its own timer, if any'
complete -c hs -n "__fish_hs_using_subcommand task; and not __fish_seen_subcommand_from add list show set done reopen archive next help" -f -a "reopen" -d 'Reopen a task without starting a timer'
complete -c hs -n "__fish_hs_using_subcommand task; and not __fish_seen_subcommand_from add list show set done reopen archive next help" -f -a "archive" -d 'Hide a task while preserving history and stopping its own timer'
complete -c hs -n "__fish_hs_using_subcommand task; and not __fish_seen_subcommand_from add list show set done reopen archive next help" -f -a "next" -d 'Suggest an open Do task, then Plan, ordered by due date and ID'
complete -c hs -n "__fish_hs_using_subcommand task; and not __fish_seen_subcommand_from add list show set done reopen archive next help" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c hs -n "__fish_hs_using_subcommand task; and __fish_seen_subcommand_from add" -l time-label -d 'Optional plain-text label for time reports; initially defaults to the task title' -r
complete -c hs -n "__fish_hs_using_subcommand task; and __fish_seen_subcommand_from add" -l category -d 'Optional grouping, e.g. Work or Personal; overrides a category in --activity' -r
complete -c hs -n "__fish_hs_using_subcommand task; and __fish_seen_subcommand_from add" -l activity -d 'Hamster-style time label and metadata, e.g. \'Writing@Work, #client\'' -r
complete -c hs -n "__fish_hs_using_subcommand task; and __fish_seen_subcommand_from add" -l notes -r
complete -c hs -n "__fish_hs_using_subcommand task; and __fish_seen_subcommand_from add" -l tags -d 'Comma-separated tags' -r
complete -c hs -n "__fish_hs_using_subcommand task; and __fish_seen_subcommand_from add" -l quadrant -r -f -a "do\t''
plan\t''
delegate\t''
drop\t''
inbox\t''"
complete -c hs -n "__fish_hs_using_subcommand task; and __fish_seen_subcommand_from add" -l due -d 'Calendar deadline YYYY-MM-DD; never changes urgency automatically' -r
complete -c hs -n "__fish_hs_using_subcommand task; and __fish_seen_subcommand_from add" -l db -d 'Use a separate hs database' -r -F
complete -c hs -n "__fish_hs_using_subcommand task; and __fish_seen_subcommand_from add" -l json -d 'Print versioned JSON for status, reports, matrix, task list/show/next, or pomo status/stats'
complete -c hs -n "__fish_hs_using_subcommand task; and __fish_seen_subcommand_from add" -s h -l help -d 'Print help'
complete -c hs -n "__fish_hs_using_subcommand task; and __fish_seen_subcommand_from list" -l db -d 'Use a separate hs database' -r -F
complete -c hs -n "__fish_hs_using_subcommand task; and __fish_seen_subcommand_from list" -l all
complete -c hs -n "__fish_hs_using_subcommand task; and __fish_seen_subcommand_from list" -l json -d 'Print versioned JSON for status, reports, matrix, task list/show/next, or pomo status/stats'
complete -c hs -n "__fish_hs_using_subcommand task; and __fish_seen_subcommand_from list" -s h -l help -d 'Print help'
complete -c hs -n "__fish_hs_using_subcommand task; and __fish_seen_subcommand_from show" -l db -d 'Use a separate hs database' -r -F
complete -c hs -n "__fish_hs_using_subcommand task; and __fish_seen_subcommand_from show" -l json -d 'Print versioned JSON for status, reports, matrix, task list/show/next, or pomo status/stats'
complete -c hs -n "__fish_hs_using_subcommand task; and __fish_seen_subcommand_from show" -s h -l help -d 'Print help'
complete -c hs -n "__fish_hs_using_subcommand task; and __fish_seen_subcommand_from set" -l title -r
complete -c hs -n "__fish_hs_using_subcommand task; and __fish_seen_subcommand_from set" -l time-label -d 'Change only the plain-text label for future time reports; preserves category, notes and tags' -r
complete -c hs -n "__fish_hs_using_subcommand task; and __fish_seen_subcommand_from set" -l category -d 'Change the grouping without changing the time label; empty text clears it' -r
complete -c hs -n "__fish_hs_using_subcommand task; and __fish_seen_subcommand_from set" -l activity -d 'Hamster-style label/category and optional notes/tags; omitted notes and tags are preserved' -r
complete -c hs -n "__fish_hs_using_subcommand task; and __fish_seen_subcommand_from set" -l notes -d 'Empty text clears the notes' -r
complete -c hs -n "__fish_hs_using_subcommand task; and __fish_seen_subcommand_from set" -l tags -d 'Replace tags with a comma-separated list; empty clears them' -r
complete -c hs -n "__fish_hs_using_subcommand task; and __fish_seen_subcommand_from set" -l quadrant -r -f -a "do\t''
plan\t''
delegate\t''
drop\t''
inbox\t''"
complete -c hs -n "__fish_hs_using_subcommand task; and __fish_seen_subcommand_from set" -l due -d 'YYYY-MM-DD, or \'none\' to clear the deadline' -r
complete -c hs -n "__fish_hs_using_subcommand task; and __fish_seen_subcommand_from set" -l db -d 'Use a separate hs database' -r -F
complete -c hs -n "__fish_hs_using_subcommand task; and __fish_seen_subcommand_from set" -l json -d 'Print versioned JSON for status, reports, matrix, task list/show/next, or pomo status/stats'
complete -c hs -n "__fish_hs_using_subcommand task; and __fish_seen_subcommand_from set" -s h -l help -d 'Print help'
complete -c hs -n "__fish_hs_using_subcommand task; and __fish_seen_subcommand_from done" -l db -d 'Use a separate hs database' -r -F
complete -c hs -n "__fish_hs_using_subcommand task; and __fish_seen_subcommand_from done" -l json -d 'Print versioned JSON for status, reports, matrix, task list/show/next, or pomo status/stats'
complete -c hs -n "__fish_hs_using_subcommand task; and __fish_seen_subcommand_from done" -s h -l help -d 'Print help'
complete -c hs -n "__fish_hs_using_subcommand task; and __fish_seen_subcommand_from reopen" -l db -d 'Use a separate hs database' -r -F
complete -c hs -n "__fish_hs_using_subcommand task; and __fish_seen_subcommand_from reopen" -l json -d 'Print versioned JSON for status, reports, matrix, task list/show/next, or pomo status/stats'
complete -c hs -n "__fish_hs_using_subcommand task; and __fish_seen_subcommand_from reopen" -s h -l help -d 'Print help'
complete -c hs -n "__fish_hs_using_subcommand task; and __fish_seen_subcommand_from archive" -l db -d 'Use a separate hs database' -r -F
complete -c hs -n "__fish_hs_using_subcommand task; and __fish_seen_subcommand_from archive" -l json -d 'Print versioned JSON for status, reports, matrix, task list/show/next, or pomo status/stats'
complete -c hs -n "__fish_hs_using_subcommand task; and __fish_seen_subcommand_from archive" -s h -l help -d 'Print help'
complete -c hs -n "__fish_hs_using_subcommand task; and __fish_seen_subcommand_from next" -l db -d 'Use a separate hs database' -r -F
complete -c hs -n "__fish_hs_using_subcommand task; and __fish_seen_subcommand_from next" -l json -d 'Print versioned JSON for status, reports, matrix, task list/show/next, or pomo status/stats'
complete -c hs -n "__fish_hs_using_subcommand task; and __fish_seen_subcommand_from next" -s h -l help -d 'Print help'
complete -c hs -n "__fish_hs_using_subcommand task; and __fish_seen_subcommand_from help" -f -a "add" -d 'Capture a finishable outcome; unclassified tasks enter the inbox'
complete -c hs -n "__fish_hs_using_subcommand task; and __fish_seen_subcommand_from help" -f -a "list" -d 'List open tasks, or include done and archived tasks with --all'
complete -c hs -n "__fish_hs_using_subcommand task; and __fish_seen_subcommand_from help" -f -a "show" -d 'Show task details, recorded time and current timer'
complete -c hs -n "__fish_hs_using_subcommand task; and __fish_seen_subcommand_from help" -f -a "set" -d 'Change task details without interrupting current work'
complete -c hs -n "__fish_hs_using_subcommand task; and __fish_seen_subcommand_from help" -f -a "done" -d 'Complete a task and stop its own timer, if any'
complete -c hs -n "__fish_hs_using_subcommand task; and __fish_seen_subcommand_from help" -f -a "reopen" -d 'Reopen a task without starting a timer'
complete -c hs -n "__fish_hs_using_subcommand task; and __fish_seen_subcommand_from help" -f -a "archive" -d 'Hide a task while preserving history and stopping its own timer'
complete -c hs -n "__fish_hs_using_subcommand task; and __fish_seen_subcommand_from help" -f -a "next" -d 'Suggest an open Do task, then Plan, ordered by due date and ID'
complete -c hs -n "__fish_hs_using_subcommand task; and __fish_seen_subcommand_from help" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c hs -n "__fish_hs_using_subcommand matrix" -l db -d 'Use a separate hs database' -r -F
complete -c hs -n "__fish_hs_using_subcommand matrix" -l json -d 'Print versioned JSON for status, reports, matrix, task list/show/next, or pomo status/stats'
complete -c hs -n "__fish_hs_using_subcommand matrix" -s h -l help -d 'Print help'
complete -c hs -n "__fish_hs_using_subcommand tui" -l db -d 'Use a separate hs database' -r -F
complete -c hs -n "__fish_hs_using_subcommand tui" -l no-sound -l no-bell -d 'Keep completion banners without playing sounds'
complete -c hs -n "__fish_hs_using_subcommand tui" -l json -d 'Print versioned JSON for status, reports, matrix, task list/show/next, or pomo status/stats'
complete -c hs -n "__fish_hs_using_subcommand tui" -s h -l help -d 'Print help'
complete -c hs -n "__fish_hs_using_subcommand ui" -l db -d 'Use a separate hs database' -r -F
complete -c hs -n "__fish_hs_using_subcommand ui" -l no-sound -l no-bell -d 'Keep completion banners without playing sounds'
complete -c hs -n "__fish_hs_using_subcommand ui" -l json -d 'Print versioned JSON for status, reports, matrix, task list/show/next, or pomo status/stats'
complete -c hs -n "__fish_hs_using_subcommand ui" -s h -l help -d 'Print help'
complete -c hs -n "__fish_hs_using_subcommand pomo; and not __fish_seen_subcommand_from start status pause resume next stop watch stats help" -l db -d 'Use a separate hs database' -r -F
complete -c hs -n "__fish_hs_using_subcommand pomo; and not __fish_seen_subcommand_from start status pause resume next stop watch stats help" -l json -d 'Print versioned JSON for status, reports, matrix, task list/show/next, or pomo status/stats'
complete -c hs -n "__fish_hs_using_subcommand pomo; and not __fish_seen_subcommand_from start status pause resume next stop watch stats help" -s h -l help -d 'Print help'
complete -c hs -n "__fish_hs_using_subcommand pomo; and not __fish_seen_subcommand_from start status pause resume next stop watch stats help" -f -a "start" -d 'Begin focus and watch it in an interactive terminal'
complete -c hs -n "__fish_hs_using_subcommand pomo; and not __fish_seen_subcommand_from start status pause resume next stop watch stats help" -f -a "status" -d 'Show phase, remaining time and completed focus count'
complete -c hs -n "__fish_hs_using_subcommand pomo; and not __fish_seen_subcommand_from start status pause resume next stop watch stats help" -f -a "pause" -d 'Pause the current phase and its work entry'
complete -c hs -n "__fish_hs_using_subcommand pomo; and not __fish_seen_subcommand_from start status pause resume next stop watch stats help" -f -a "resume" -d 'Resume the paused phase with its remaining time'
complete -c hs -n "__fish_hs_using_subcommand pomo; and not __fish_seen_subcommand_from start status pause resume next stop watch stats help" -f -a "next" -d 'Start the offered break or next focus period'
complete -c hs -n "__fish_hs_using_subcommand pomo; and not __fish_seen_subcommand_from start status pause resume next stop watch stats help" -f -a "stop" -d 'End the session, preserving work already recorded'
complete -c hs -n "__fish_hs_using_subcommand pomo; and not __fish_seen_subcommand_from start status pause resume next stop watch stats help" -f -a "watch" -d 'Show a live countdown until the phase ends or pauses; Ctrl-C leaves it running'
complete -c hs -n "__fish_hs_using_subcommand pomo; and not __fish_seen_subcommand_from start status pause resume next stop watch stats help" -f -a "stats" -d 'Show completed/interrupted focus periods and recorded focus/break totals'
complete -c hs -n "__fish_hs_using_subcommand pomo; and not __fish_seen_subcommand_from start status pause resume next stop watch stats help" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c hs -n "__fish_hs_using_subcommand pomo; and __fish_seen_subcommand_from start" -l task -d 'Focus on an open task by its ID' -r
complete -c hs -n "__fish_hs_using_subcommand pomo; and __fish_seen_subcommand_from start" -l focus -d 'Focus duration, e.g. 25m, 90s, 1h (bare numbers are minutes)' -r
complete -c hs -n "__fish_hs_using_subcommand pomo; and __fish_seen_subcommand_from start" -l break -d 'Short break duration' -r
complete -c hs -n "__fish_hs_using_subcommand pomo; and __fish_seen_subcommand_from start" -l long-break -d 'Long break duration' -r
complete -c hs -n "__fish_hs_using_subcommand pomo; and __fish_seen_subcommand_from start" -l rounds -d 'Completed focus periods before each long break' -r
complete -c hs -n "__fish_hs_using_subcommand pomo; and __fish_seen_subcommand_from start" -l db -d 'Use a separate hs database' -r -F
complete -c hs -n "__fish_hs_using_subcommand pomo; and __fish_seen_subcommand_from start" -l detach -d 'Return immediately instead of watching in an interactive terminal'
complete -c hs -n "__fish_hs_using_subcommand pomo; and __fish_seen_subcommand_from start" -l no-bell -l no-sound -d 'Mute the completion sound; keep the visual cue'
complete -c hs -n "__fish_hs_using_subcommand pomo; and __fish_seen_subcommand_from start" -l json -d 'Print versioned JSON for status, reports, matrix, task list/show/next, or pomo status/stats'
complete -c hs -n "__fish_hs_using_subcommand pomo; and __fish_seen_subcommand_from start" -s h -l help -d 'Print help'
complete -c hs -n "__fish_hs_using_subcommand pomo; and __fish_seen_subcommand_from status" -l db -d 'Use a separate hs database' -r -F
complete -c hs -n "__fish_hs_using_subcommand pomo; and __fish_seen_subcommand_from status" -l json -d 'Print versioned JSON for status, reports, matrix, task list/show/next, or pomo status/stats'
complete -c hs -n "__fish_hs_using_subcommand pomo; and __fish_seen_subcommand_from status" -s h -l help -d 'Print help'
complete -c hs -n "__fish_hs_using_subcommand pomo; and __fish_seen_subcommand_from pause" -l db -d 'Use a separate hs database' -r -F
complete -c hs -n "__fish_hs_using_subcommand pomo; and __fish_seen_subcommand_from pause" -l json -d 'Print versioned JSON for status, reports, matrix, task list/show/next, or pomo status/stats'
complete -c hs -n "__fish_hs_using_subcommand pomo; and __fish_seen_subcommand_from pause" -s h -l help -d 'Print help'
complete -c hs -n "__fish_hs_using_subcommand pomo; and __fish_seen_subcommand_from resume" -l db -d 'Use a separate hs database' -r -F
complete -c hs -n "__fish_hs_using_subcommand pomo; and __fish_seen_subcommand_from resume" -l detach -d 'Return immediately instead of watching in an interactive terminal'
complete -c hs -n "__fish_hs_using_subcommand pomo; and __fish_seen_subcommand_from resume" -l no-bell -l no-sound -d 'Mute the completion sound; keep the visual cue'
complete -c hs -n "__fish_hs_using_subcommand pomo; and __fish_seen_subcommand_from resume" -l json -d 'Print versioned JSON for status, reports, matrix, task list/show/next, or pomo status/stats'
complete -c hs -n "__fish_hs_using_subcommand pomo; and __fish_seen_subcommand_from resume" -s h -l help -d 'Print help'
complete -c hs -n "__fish_hs_using_subcommand pomo; and __fish_seen_subcommand_from next" -l db -d 'Use a separate hs database' -r -F
complete -c hs -n "__fish_hs_using_subcommand pomo; and __fish_seen_subcommand_from next" -l detach -d 'Return immediately instead of watching in an interactive terminal'
complete -c hs -n "__fish_hs_using_subcommand pomo; and __fish_seen_subcommand_from next" -l no-bell -l no-sound -d 'Mute the completion sound; keep the visual cue'
complete -c hs -n "__fish_hs_using_subcommand pomo; and __fish_seen_subcommand_from next" -l json -d 'Print versioned JSON for status, reports, matrix, task list/show/next, or pomo status/stats'
complete -c hs -n "__fish_hs_using_subcommand pomo; and __fish_seen_subcommand_from next" -s h -l help -d 'Print help'
complete -c hs -n "__fish_hs_using_subcommand pomo; and __fish_seen_subcommand_from stop" -l db -d 'Use a separate hs database' -r -F
complete -c hs -n "__fish_hs_using_subcommand pomo; and __fish_seen_subcommand_from stop" -l json -d 'Print versioned JSON for status, reports, matrix, task list/show/next, or pomo status/stats'
complete -c hs -n "__fish_hs_using_subcommand pomo; and __fish_seen_subcommand_from stop" -s h -l help -d 'Print help'
complete -c hs -n "__fish_hs_using_subcommand pomo; and __fish_seen_subcommand_from watch" -l db -d 'Use a separate hs database' -r -F
complete -c hs -n "__fish_hs_using_subcommand pomo; and __fish_seen_subcommand_from watch" -l no-bell -l no-sound -d 'Mute the completion sound; keep the visual cue'
complete -c hs -n "__fish_hs_using_subcommand pomo; and __fish_seen_subcommand_from watch" -l json -d 'Print versioned JSON for status, reports, matrix, task list/show/next, or pomo status/stats'
complete -c hs -n "__fish_hs_using_subcommand pomo; and __fish_seen_subcommand_from watch" -s h -l help -d 'Print help'
complete -c hs -n "__fish_hs_using_subcommand pomo; and __fish_seen_subcommand_from stats" -l db -d 'Use a separate hs database' -r -F
complete -c hs -n "__fish_hs_using_subcommand pomo; and __fish_seen_subcommand_from stats" -l json -d 'Print versioned JSON for status, reports, matrix, task list/show/next, or pomo status/stats'
complete -c hs -n "__fish_hs_using_subcommand pomo; and __fish_seen_subcommand_from stats" -s h -l help -d 'Print help'
complete -c hs -n "__fish_hs_using_subcommand pomo; and __fish_seen_subcommand_from help" -f -a "start" -d 'Begin focus and watch it in an interactive terminal'
complete -c hs -n "__fish_hs_using_subcommand pomo; and __fish_seen_subcommand_from help" -f -a "status" -d 'Show phase, remaining time and completed focus count'
complete -c hs -n "__fish_hs_using_subcommand pomo; and __fish_seen_subcommand_from help" -f -a "pause" -d 'Pause the current phase and its work entry'
complete -c hs -n "__fish_hs_using_subcommand pomo; and __fish_seen_subcommand_from help" -f -a "resume" -d 'Resume the paused phase with its remaining time'
complete -c hs -n "__fish_hs_using_subcommand pomo; and __fish_seen_subcommand_from help" -f -a "next" -d 'Start the offered break or next focus period'
complete -c hs -n "__fish_hs_using_subcommand pomo; and __fish_seen_subcommand_from help" -f -a "stop" -d 'End the session, preserving work already recorded'
complete -c hs -n "__fish_hs_using_subcommand pomo; and __fish_seen_subcommand_from help" -f -a "watch" -d 'Show a live countdown until the phase ends or pauses; Ctrl-C leaves it running'
complete -c hs -n "__fish_hs_using_subcommand pomo; and __fish_seen_subcommand_from help" -f -a "stats" -d 'Show completed/interrupted focus periods and recorded focus/break totals'
complete -c hs -n "__fish_hs_using_subcommand pomo; and __fish_seen_subcommand_from help" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c hs -n "__fish_hs_using_subcommand start" -l task -d 'Track an open task by its ID' -r
complete -c hs -n "__fish_hs_using_subcommand start" -l db -d 'Use a separate hs database' -r -F
complete -c hs -n "__fish_hs_using_subcommand start" -l json -d 'Print versioned JSON for status, reports, matrix, task list/show/next, or pomo status/stats'
complete -c hs -n "__fish_hs_using_subcommand start" -s h -l help -d 'Print help'
complete -c hs -n "__fish_hs_using_subcommand add" -l db -d 'Use a separate hs database' -r -F
complete -c hs -n "__fish_hs_using_subcommand add" -l json -d 'Print versioned JSON for status, reports, matrix, task list/show/next, or pomo status/stats'
complete -c hs -n "__fish_hs_using_subcommand add" -s h -l help -d 'Print help'
complete -c hs -n "__fish_hs_using_subcommand status" -l db -d 'Use a separate hs database' -r -F
complete -c hs -n "__fish_hs_using_subcommand status" -l json -d 'Print versioned JSON for status, reports, matrix, task list/show/next, or pomo status/stats'
complete -c hs -n "__fish_hs_using_subcommand status" -s h -l help -d 'Print help'
complete -c hs -n "__fish_hs_using_subcommand stop" -l at -d 'Stop at a clock time, dated time, or signed minute offset' -r
complete -c hs -n "__fish_hs_using_subcommand stop" -l db -d 'Use a separate hs database' -r -F
complete -c hs -n "__fish_hs_using_subcommand stop" -l json -d 'Print versioned JSON for status, reports, matrix, task list/show/next, or pomo status/stats'
complete -c hs -n "__fish_hs_using_subcommand stop" -s h -l help -d 'Print help'
complete -c hs -n "__fish_hs_using_subcommand resume" -l db -d 'Use a separate hs database' -r -F
complete -c hs -n "__fish_hs_using_subcommand resume" -l json -d 'Print versioned JSON for status, reports, matrix, task list/show/next, or pomo status/stats'
complete -c hs -n "__fish_hs_using_subcommand resume" -s h -l help -d 'Print help'
complete -c hs -n "__fish_hs_using_subcommand edit" -l start -r
complete -c hs -n "__fish_hs_using_subcommand edit" -l end -r
complete -c hs -n "__fish_hs_using_subcommand edit" -l activity -r
complete -c hs -n "__fish_hs_using_subcommand edit" -l category -d 'Use an empty string to remove the category' -r
complete -c hs -n "__fish_hs_using_subcommand edit" -l description -r
complete -c hs -n "__fish_hs_using_subcommand edit" -l tags -d 'Replace tags with a comma-separated list; empty clears tags' -r
complete -c hs -n "__fish_hs_using_subcommand edit" -l db -d 'Use a separate hs database' -r -F
complete -c hs -n "__fish_hs_using_subcommand edit" -l json -d 'Print versioned JSON for status, reports, matrix, task list/show/next, or pomo status/stats'
complete -c hs -n "__fish_hs_using_subcommand edit" -s h -l help -d 'Print help'
complete -c hs -n "__fish_hs_using_subcommand delete" -l db -d 'Use a separate hs database' -r -F
complete -c hs -n "__fish_hs_using_subcommand delete" -l json -d 'Print versioned JSON for status, reports, matrix, task list/show/next, or pomo status/stats'
complete -c hs -n "__fish_hs_using_subcommand delete" -s h -l help -d 'Print help'
complete -c hs -n "__fish_hs_using_subcommand list" -l from -d 'First workday, YYYY-MM-DD' -r
complete -c hs -n "__fish_hs_using_subcommand list" -l to -d 'Last workday inclusive, YYYY-MM-DD; defaults to today' -r
complete -c hs -n "__fish_hs_using_subcommand list" -l search -d 'Search activity, category, description, and tags' -r
complete -c hs -n "__fish_hs_using_subcommand list" -l db -d 'Use a separate hs database' -r -F
complete -c hs -n "__fish_hs_using_subcommand list" -l today -d 'Current workday (the default)'
complete -c hs -n "__fish_hs_using_subcommand list" -l week -d 'Current Monday-based week'
complete -c hs -n "__fish_hs_using_subcommand list" -l month -d 'Current calendar month'
complete -c hs -n "__fish_hs_using_subcommand list" -l all -d 'All recorded history'
complete -c hs -n "__fish_hs_using_subcommand list" -l json -d 'Print versioned JSON for status, reports, matrix, task list/show/next, or pomo status/stats'
complete -c hs -n "__fish_hs_using_subcommand list" -s h -l help -d 'Print help'
complete -c hs -n "__fish_hs_using_subcommand report" -l from -d 'First workday, YYYY-MM-DD' -r
complete -c hs -n "__fish_hs_using_subcommand report" -l to -d 'Last workday inclusive, YYYY-MM-DD; defaults to today' -r
complete -c hs -n "__fish_hs_using_subcommand report" -l group-by -r -f -a "activity\t''
category\t''
tag\t''
day\t''
quadrant\t''"
complete -c hs -n "__fish_hs_using_subcommand report" -l db -d 'Use a separate hs database' -r -F
complete -c hs -n "__fish_hs_using_subcommand report" -l today -d 'Current workday (the default)'
complete -c hs -n "__fish_hs_using_subcommand report" -l week -d 'Current Monday-based week'
complete -c hs -n "__fish_hs_using_subcommand report" -l month -d 'Current calendar month'
complete -c hs -n "__fish_hs_using_subcommand report" -l all -d 'All recorded history'
complete -c hs -n "__fish_hs_using_subcommand report" -l json -d 'Print versioned JSON for status, reports, matrix, task list/show/next, or pomo status/stats'
complete -c hs -n "__fish_hs_using_subcommand report" -s h -l help -d 'Print help'
complete -c hs -n "__fish_hs_using_subcommand export" -l from -d 'First workday, YYYY-MM-DD' -r
complete -c hs -n "__fish_hs_using_subcommand export" -l to -d 'Last workday inclusive, YYYY-MM-DD; defaults to today' -r
complete -c hs -n "__fish_hs_using_subcommand export" -l format -r -f -a "tsv\t''
json\t''"
complete -c hs -n "__fish_hs_using_subcommand export" -l db -d 'Use a separate hs database' -r -F
complete -c hs -n "__fish_hs_using_subcommand export" -l today -d 'Current workday (the default)'
complete -c hs -n "__fish_hs_using_subcommand export" -l week -d 'Current Monday-based week'
complete -c hs -n "__fish_hs_using_subcommand export" -l month -d 'Current calendar month'
complete -c hs -n "__fish_hs_using_subcommand export" -l all -d 'All recorded history'
complete -c hs -n "__fish_hs_using_subcommand export" -l json -d 'Print versioned JSON for status, reports, matrix, task list/show/next, or pomo status/stats'
complete -c hs -n "__fish_hs_using_subcommand export" -s h -l help -d 'Print help'
complete -c hs -n "__fish_hs_using_subcommand import; and not __fish_seen_subcommand_from hamster help" -l db -d 'Use a separate hs database' -r -F
complete -c hs -n "__fish_hs_using_subcommand import; and not __fish_seen_subcommand_from hamster help" -l json -d 'Print versioned JSON for status, reports, matrix, task list/show/next, or pomo status/stats'
complete -c hs -n "__fish_hs_using_subcommand import; and not __fish_seen_subcommand_from hamster help" -s h -l help -d 'Print help'
complete -c hs -n "__fish_hs_using_subcommand import; and not __fish_seen_subcommand_from hamster help" -f -a "hamster" -d 'Import a Hamster schema-v9 database'
complete -c hs -n "__fish_hs_using_subcommand import; and not __fish_seen_subcommand_from hamster help" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c hs -n "__fish_hs_using_subcommand import; and __fish_seen_subcommand_from hamster" -l source-name -d 'Stable identity for this source; reuse after moving its file' -r
complete -c hs -n "__fish_hs_using_subcommand import; and __fish_seen_subcommand_from hamster" -l day-start -d 'Original Hamster workday boundary, HH:MM' -r
complete -c hs -n "__fish_hs_using_subcommand import; and __fish_seen_subcommand_from hamster" -l db -d 'Use a separate hs database' -r -F
complete -c hs -n "__fish_hs_using_subcommand import; and __fish_seen_subcommand_from hamster" -l dry-run -d 'Validate and preview without committing imported records'
complete -c hs -n "__fish_hs_using_subcommand import; and __fish_seen_subcommand_from hamster" -l json -d 'Print versioned JSON for status, reports, matrix, task list/show/next, or pomo status/stats'
complete -c hs -n "__fish_hs_using_subcommand import; and __fish_seen_subcommand_from hamster" -s h -l help -d 'Print help'
complete -c hs -n "__fish_hs_using_subcommand import; and __fish_seen_subcommand_from help" -f -a "hamster" -d 'Import a Hamster schema-v9 database'
complete -c hs -n "__fish_hs_using_subcommand import; and __fish_seen_subcommand_from help" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c hs -n "__fish_hs_using_subcommand config; and not __fish_seen_subcommand_from day-start help" -l db -d 'Use a separate hs database' -r -F
complete -c hs -n "__fish_hs_using_subcommand config; and not __fish_seen_subcommand_from day-start help" -l json -d 'Print versioned JSON for status, reports, matrix, task list/show/next, or pomo status/stats'
complete -c hs -n "__fish_hs_using_subcommand config; and not __fish_seen_subcommand_from day-start help" -s h -l help -d 'Print help'
complete -c hs -n "__fish_hs_using_subcommand config; and not __fish_seen_subcommand_from day-start help" -f -a "day-start" -d 'Print or set the start of a workday (default 00:00)'
complete -c hs -n "__fish_hs_using_subcommand config; and not __fish_seen_subcommand_from day-start help" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c hs -n "__fish_hs_using_subcommand config; and __fish_seen_subcommand_from day-start" -l db -d 'Use a separate hs database' -r -F
complete -c hs -n "__fish_hs_using_subcommand config; and __fish_seen_subcommand_from day-start" -l json -d 'Print versioned JSON for status, reports, matrix, task list/show/next, or pomo status/stats'
complete -c hs -n "__fish_hs_using_subcommand config; and __fish_seen_subcommand_from day-start" -s h -l help -d 'Print help'
complete -c hs -n "__fish_hs_using_subcommand config; and __fish_seen_subcommand_from help" -f -a "day-start" -d 'Print or set the start of a workday (default 00:00)'
complete -c hs -n "__fish_hs_using_subcommand config; and __fish_seen_subcommand_from help" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c hs -n "__fish_hs_using_subcommand backup" -l db -d 'Use a separate hs database' -r -F
complete -c hs -n "__fish_hs_using_subcommand backup" -l json -d 'Print versioned JSON for status, reports, matrix, task list/show/next, or pomo status/stats'
complete -c hs -n "__fish_hs_using_subcommand backup" -s h -l help -d 'Print help'
complete -c hs -n "__fish_hs_using_subcommand activities" -l db -d 'Use a separate hs database' -r -F
complete -c hs -n "__fish_hs_using_subcommand activities" -l json -d 'Print versioned JSON for status, reports, matrix, task list/show/next, or pomo status/stats'
complete -c hs -n "__fish_hs_using_subcommand activities" -s h -l help -d 'Print help'
complete -c hs -n "__fish_hs_using_subcommand categories" -l db -d 'Use a separate hs database' -r -F
complete -c hs -n "__fish_hs_using_subcommand categories" -l json -d 'Print versioned JSON for status, reports, matrix, task list/show/next, or pomo status/stats'
complete -c hs -n "__fish_hs_using_subcommand categories" -s h -l help -d 'Print help'
complete -c hs -n "__fish_hs_using_subcommand tags" -l db -d 'Use a separate hs database' -r -F
complete -c hs -n "__fish_hs_using_subcommand tags" -l json -d 'Print versioned JSON for status, reports, matrix, task list/show/next, or pomo status/stats'
complete -c hs -n "__fish_hs_using_subcommand tags" -s h -l help -d 'Print help'
complete -c hs -n "__fish_hs_using_subcommand completions" -l db -d 'Use a separate hs database' -r -F
complete -c hs -n "__fish_hs_using_subcommand completions" -l json -d 'Print versioned JSON for status, reports, matrix, task list/show/next, or pomo status/stats'
complete -c hs -n "__fish_hs_using_subcommand completions" -s h -l help -d 'Print help'
complete -c hs -n "__fish_hs_using_subcommand help; and not __fish_seen_subcommand_from api capabilities context mcp task matrix tui pomo start add status stop resume edit delete list report export import config backup activities categories tags completions help" -f -a "api" -d 'Execute one versioned JSON request from stdin; emit one JSON response'
complete -c hs -n "__fish_hs_using_subcommand help; and not __fish_seen_subcommand_from api capabilities context mcp task matrix tui pomo start add status stop resume edit delete list report export import config backup activities categories tags completions help" -f -a "capabilities" -d 'Print agent operations and their JSON Schemas without opening a database'
complete -c hs -n "__fish_hs_using_subcommand help; and not __fish_seen_subcommand_from api capabilities context mcp task matrix tui pomo start add status stop resume edit delete list report export import config backup activities categories tags completions help" -f -a "context" -d 'Inspect stored state as JSON without advancing timers or migrating data'
complete -c hs -n "__fish_hs_using_subcommand help; and not __fish_seen_subcommand_from api capabilities context mcp task matrix tui pomo start add status stop resume edit delete list report export import config backup activities categories tags completions help" -f -a "mcp" -d 'Serve local agent tools over MCP stdio (protocol 2025-11-25)'
complete -c hs -n "__fish_hs_using_subcommand help; and not __fish_seen_subcommand_from api capabilities context mcp task matrix tui pomo start add status stop resume edit delete list report export import config backup activities categories tags completions help" -f -a "task" -d 'Capture and prioritize tasks'
complete -c hs -n "__fish_hs_using_subcommand help; and not __fish_seen_subcommand_from api capabilities context mcp task matrix tui pomo start add status stop resume edit delete list report export import config backup activities categories tags completions help" -f -a "matrix" -d 'Show open tasks by importance and urgency, followed by the inbox'
complete -c hs -n "__fish_hs_using_subcommand help; and not __fish_seen_subcommand_from api capabilities context mcp task matrix tui pomo start add status stop resume edit delete list report export import config backup activities categories tags completions help" -f -a "tui" -d 'Open the interactive matrix, inbox, task editor and timer controls'
complete -c hs -n "__fish_hs_using_subcommand help; and not __fish_seen_subcommand_from api capabilities context mcp task matrix tui pomo start add status stop resume edit delete list report export import config backup activities categories tags completions help" -f -a "pomo" -d 'Run persistent focus and break timers'
complete -c hs -n "__fish_hs_using_subcommand help; and not __fish_seen_subcommand_from api capabilities context mcp task matrix tui pomo start add status stop resume edit delete list report export import config backup activities categories tags completions help" -f -a "start" -d 'Start an activity, closing the current one at the same time'
complete -c hs -n "__fish_hs_using_subcommand help; and not __fish_seen_subcommand_from api capabilities context mcp task matrix tui pomo start add status stop resume edit delete list report export import config backup activities categories tags completions help" -f -a "add" -d 'Record a past interval or start an activity with no end time'
complete -c hs -n "__fish_hs_using_subcommand help; and not __fish_seen_subcommand_from api capabilities context mcp task matrix tui pomo start add status stop resume edit delete list report export import config backup activities categories tags completions help" -f -a "status" -d 'Show the current activity'
complete -c hs -n "__fish_hs_using_subcommand help; and not __fish_seen_subcommand_from api capabilities context mcp task matrix tui pomo start add status stop resume edit delete list report export import config backup activities categories tags completions help" -f -a "stop" -d 'Stop the current activity'
complete -c hs -n "__fish_hs_using_subcommand help; and not __fish_seen_subcommand_from api capabilities context mcp task matrix tui pomo start add status stop resume edit delete list report export import config backup activities categories tags completions help" -f -a "resume" -d 'Start a new entry using a previous entry\'s details'
complete -c hs -n "__fish_hs_using_subcommand help; and not __fish_seen_subcommand_from api capabilities context mcp task matrix tui pomo start add status stop resume edit delete list report export import config backup activities categories tags completions help" -f -a "edit" -d 'Correct an entry by its ID'
complete -c hs -n "__fish_hs_using_subcommand help; and not __fish_seen_subcommand_from api capabilities context mcp task matrix tui pomo start add status stop resume edit delete list report export import config backup activities categories tags completions help" -f -a "delete" -d 'Delete an entry by its ID'
complete -c hs -n "__fish_hs_using_subcommand help; and not __fish_seen_subcommand_from api capabilities context mcp task matrix tui pomo start add status stop resume edit delete list report export import config backup activities categories tags completions help" -f -a "list" -d 'List entries, including unfinished imported records'
complete -c hs -n "__fish_hs_using_subcommand help; and not __fish_seen_subcommand_from api capabilities context mcp task matrix tui pomo start add status stop resume edit delete list report export import config backup activities categories tags completions help" -f -a "report" -d 'Show totals for a day, week, month, or date range'
complete -c hs -n "__fish_hs_using_subcommand help; and not __fish_seen_subcommand_from api capabilities context mcp task matrix tui pomo start add status stop resume edit delete list report export import config backup activities categories tags completions help" -f -a "export" -d 'Export matching entries and clipped durations'
complete -c hs -n "__fish_hs_using_subcommand help; and not __fish_seen_subcommand_from api capabilities context mcp task matrix tui pomo start add status stop resume edit delete list report export import config backup activities categories tags completions help" -f -a "import" -d 'Import history from another time tracker'
complete -c hs -n "__fish_hs_using_subcommand help; and not __fish_seen_subcommand_from api capabilities context mcp task matrix tui pomo start add status stop resume edit delete list report export import config backup activities categories tags completions help" -f -a "config" -d 'View or change workday settings'
complete -c hs -n "__fish_hs_using_subcommand help; and not __fish_seen_subcommand_from api capabilities context mcp task matrix tui pomo start add status stop resume edit delete list report export import config backup activities categories tags completions help" -f -a "backup" -d 'Create a consistent SQLite backup at a new path'
complete -c hs -n "__fish_hs_using_subcommand help; and not __fish_seen_subcommand_from api capabilities context mcp task matrix tui pomo start add status stop resume edit delete list report export import config backup activities categories tags completions help" -f -a "activities" -d 'List activities available for reuse'
complete -c hs -n "__fish_hs_using_subcommand help; and not __fish_seen_subcommand_from api capabilities context mcp task matrix tui pomo start add status stop resume edit delete list report export import config backup activities categories tags completions help" -f -a "categories" -d 'List known categories'
complete -c hs -n "__fish_hs_using_subcommand help; and not __fish_seen_subcommand_from api capabilities context mcp task matrix tui pomo start add status stop resume edit delete list report export import config backup activities categories tags completions help" -f -a "tags" -d 'List tags available for reuse'
complete -c hs -n "__fish_hs_using_subcommand help; and not __fish_seen_subcommand_from api capabilities context mcp task matrix tui pomo start add status stop resume edit delete list report export import config backup activities categories tags completions help" -f -a "completions" -d 'Print a shell completion script'
complete -c hs -n "__fish_hs_using_subcommand help; and not __fish_seen_subcommand_from api capabilities context mcp task matrix tui pomo start add status stop resume edit delete list report export import config backup activities categories tags completions help" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c hs -n "__fish_hs_using_subcommand help; and __fish_seen_subcommand_from task" -f -a "add" -d 'Capture a finishable outcome; unclassified tasks enter the inbox'
complete -c hs -n "__fish_hs_using_subcommand help; and __fish_seen_subcommand_from task" -f -a "list" -d 'List open tasks, or include done and archived tasks with --all'
complete -c hs -n "__fish_hs_using_subcommand help; and __fish_seen_subcommand_from task" -f -a "show" -d 'Show task details, recorded time and current timer'
complete -c hs -n "__fish_hs_using_subcommand help; and __fish_seen_subcommand_from task" -f -a "set" -d 'Change task details without interrupting current work'
complete -c hs -n "__fish_hs_using_subcommand help; and __fish_seen_subcommand_from task" -f -a "done" -d 'Complete a task and stop its own timer, if any'
complete -c hs -n "__fish_hs_using_subcommand help; and __fish_seen_subcommand_from task" -f -a "reopen" -d 'Reopen a task without starting a timer'
complete -c hs -n "__fish_hs_using_subcommand help; and __fish_seen_subcommand_from task" -f -a "archive" -d 'Hide a task while preserving history and stopping its own timer'
complete -c hs -n "__fish_hs_using_subcommand help; and __fish_seen_subcommand_from task" -f -a "next" -d 'Suggest an open Do task, then Plan, ordered by due date and ID'
complete -c hs -n "__fish_hs_using_subcommand help; and __fish_seen_subcommand_from pomo" -f -a "start" -d 'Begin focus and watch it in an interactive terminal'
complete -c hs -n "__fish_hs_using_subcommand help; and __fish_seen_subcommand_from pomo" -f -a "status" -d 'Show phase, remaining time and completed focus count'
complete -c hs -n "__fish_hs_using_subcommand help; and __fish_seen_subcommand_from pomo" -f -a "pause" -d 'Pause the current phase and its work entry'
complete -c hs -n "__fish_hs_using_subcommand help; and __fish_seen_subcommand_from pomo" -f -a "resume" -d 'Resume the paused phase with its remaining time'
complete -c hs -n "__fish_hs_using_subcommand help; and __fish_seen_subcommand_from pomo" -f -a "next" -d 'Start the offered break or next focus period'
complete -c hs -n "__fish_hs_using_subcommand help; and __fish_seen_subcommand_from pomo" -f -a "stop" -d 'End the session, preserving work already recorded'
complete -c hs -n "__fish_hs_using_subcommand help; and __fish_seen_subcommand_from pomo" -f -a "watch" -d 'Show a live countdown until the phase ends or pauses; Ctrl-C leaves it running'
complete -c hs -n "__fish_hs_using_subcommand help; and __fish_seen_subcommand_from pomo" -f -a "stats" -d 'Show completed/interrupted focus periods and recorded focus/break totals'
complete -c hs -n "__fish_hs_using_subcommand help; and __fish_seen_subcommand_from import" -f -a "hamster" -d 'Import a Hamster schema-v9 database'
complete -c hs -n "__fish_hs_using_subcommand help; and __fish_seen_subcommand_from config" -f -a "day-start" -d 'Print or set the start of a workday (default 00:00)'

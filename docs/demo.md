# Terminal demo

[Back to hs](../README.md)

![The hs terminal interface. A Pomodoro panel at the top shows a large 24:59 countdown for a running focus session on the fictional task "Send the Lumen Bakery menu update". Below it, an Eisenhower matrix holds one fictional task in each of Plan, Do, Drop, and Delegate, with a fifth task in the inbox and the key hints along the bottom.](../assets/demo-tui.png)

[`demo.sh`](demo.sh) walks through hs with five fictional tasks. It adds one task to each quadrant and one to the inbox, prints the matrix, starts a 25-minute focus session on the first task, and then opens the terminal interface.

## Run it

From a checkout, after building:

```sh
HS=./target/release/hs docs/demo.sh
```

With `hs` already on your `PATH`, run `docs/demo.sh` on its own. Add `--no-tui` to print the matrix and focus status without opening the interface. The interface opens only when the script runs in an interactive terminal.

In the interface, try these keys from the [user guide](guide.md#use-the-interactive-terminal-interface):

| Key | Try it |
| --- | --- |
| `Space` | Pause or resume the running focus session. |
| Arrow keys or `1` to `5` | Move between the quadrants and the inbox. |
| `j` / `k` | Select the next or previous task. |
| `Enter` | Open the selected task's details. |
| `?` | Show every key. |
| `q` | Leave the interface. |

## Setup and cleanup

The script creates a new directory with `mktemp -d` and passes `--db` with a database inside it to every command. It also points `XDG_DATA_HOME` at that directory, so even a command without `--db` could not reach your default database. The script stops the demo's focus session at the end. When it exits for any reason, including Ctrl-C, it deletes the directory and the database in it. There is nothing to clean up afterward.

All task names in the demo are fictional.

## Regenerate the screenshot

The screenshot comes from running this demo, not a mock-up. After changing the interface, rebuild and regenerate it with the [pyte](https://pypi.org/project/pyte/) terminal emulator and `rsvg-convert`:

```sh
python3 -m pip install pyte
cargo build --release --locked
python3 scripts/render-demo-screenshot.py target/release/hs assets/demo-tui.svg
rsvg-convert -z 2 assets/demo-tui.svg -o assets/demo-tui.png
```

The script runs `docs/demo.sh` in a 112 by 34 pseudo-terminal, captures the interface once it appears, and quits.

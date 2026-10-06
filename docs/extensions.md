# Writing an extension

An extension adds a task to Open Pointcloud Studio without changing the
application. It is a program in a folder of its own, in any language, that
drives the window through the [local command API](../native/API.md): it reads
what is open, shows a message, reports its progress, asks the user for a
file, opens what it wrote as a layer, and whatever else the API offers. It
runs as a process of its own, so an extension that fails or crashes ends its
own run and never the window.

An extension can add buttons to the **EXTENSIONS** group of the ribbon and
tiles to the **New** and the **Export** page of the File view. The user starts
it with one of those, or with **Run** on the **Extensions** page of the File
view; a script or an MCP client can start it with `run_extension`.

This page is for authors. The [user guide](guide.md#settings-language-and-extensions)
describes installing and managing extensions, and
[`native/extensions/examples/point-count-report`](../native/extensions/examples/point-count-report)
is a complete example in PowerShell and Python.

## The folder

An extension is a folder that holds `extension.json`, the program or script
it starts, the SVG icons of its buttons and whatever else it needs. It is
installed from that folder, from the `extension.json` in it, or from a `.zip`
archive of it; in an archive the files may lie at the top or in one folder.
The application copies the files to `extensions/<id>/` in its settings
folder (`%APPDATA%\open-pointcloud-studio-native` on Windows,
`~/.config/open-pointcloud-studio-native` elsewhere).

What is installed is checked strictly, and refused with the reason when it
does not fit:

- at most 1000 files, together at most 64 MiB, at most 12 folders deep and
  paths of at most 200 characters;
- no links of any kind, and names that every system keeps: no `:`, `\`, `*`,
  `?`, `"`, `<`, `>` or `|`, no name that ends in a dot or a space, and none
  of the device names of Windows such as `con`, `nul`, `com1` or `lpt³`,
  also not with an extension after them;
- an archive of at most 64 MiB with stored or deflated entries, without
  encryption and without ZIP64, whose entries all stay inside the folder:
  no `..`, no absolute path, no drive letter, no `\` between folders, and no
  two names that differ in letter case only. The `__MACOSX` folder that
  macOS adds is left out;
- the folders `logs` and `.git` at the top of the folder are not copied:
  `logs` holds the logs of the runs once the extension is installed.

## extension.json

```json
{
  "id": "org.example.room-report",
  "name": {"en": "Room report", "nl": "Ruimterapport"},
  "version": "1.2.0",
  "author": "Example Engineering",
  "description": "Writes the floor area of the section box to a spreadsheet.",
  "homepage": "https://example.org/room-report",
  "min_app_version": "0.9.1",
  "command": {
    "windows": {"interpreter": "powershell", "program": "report.ps1"},
    "any": {"interpreter": "python", "program": "report.py", "args": ["--quiet"]}
  },
  "uses": {
    "network": false,
    "files_outside_folder": true,
    "commands": ["status", "measure", "open"]
  },
  "contributes": {
    "ribbon": [
      {
        "id": "report",
        "label": "Room report",
        "icon": "icons/report.svg",
        "tooltip": "Write the floor area of the section box",
        "args": ["--area"]
      }
    ],
    "file_view": [
      {
        "id": "export",
        "page": "export",
        "title": "Room report…",
        "detail": "A spreadsheet with the floor area of the section box",
        "args": ["--save"]
      }
    ]
  }
}
```

The file is UTF-8, with or without a byte-order mark, and at most 64 KiB.
Every field that is not listed here is refused, so that a typing error does
not go unnoticed. A text (`name`, `description`, `label`, `tooltip`,
`title`, `detail`) is either a string or an object with a text per language
code; English, `en`, is needed, and the window shows the text of its own
language when there is one. Texts are one line without control characters.

| Field | Needed | What it is |
| --- | --- | --- |
| `id` | yes | 3 to 64 lowercase letters and digits in parts joined by single dots or dashes, such as `org.example.room-report`; the name of its folder. Not the id of a built-in feature such as `bag3d` |
| `name` | yes | Text of at most 60 characters |
| `version` | yes | Up to four numbers such as `1.2.0`, optionally with a mark before its release such as `1.2.0-beta.1`: parts of letters and digits between single dots. Versions are ordered as Semantic Versioning orders them, so `1.2.0-beta.2` comes before `1.2.0-beta.10`, which comes before `1.2.0` |
| `author` | yes | At most 100 characters |
| `description` | yes | Text of at most 500 characters |
| `homepage` | no | A web address that starts with `https://` or `http://` |
| `min_app_version` | yes | The oldest version of the application it works with; an older application refuses to install it |
| `command` | yes | How the program starts; see below |
| `uses` | yes | What it uses; see below |
| `contributes` | no | Its `ribbon` buttons and `file_view` tiles; see below |

### command

`command` is one launch for every system, or an object with a launch under
`windows`, `macos`, `linux` and `any`; the launch for the system is used,
else the one under `any`. An extension without a launch for a system cannot
be installed there. Every launch given is checked, also those for other
systems.

| Field of a launch | What it is |
| --- | --- |
| `program` | The program or script, relative to the folder with `/` between folders. It must be a file in the folder |
| `interpreter` | Optional: `python`, `powershell`, `node` or `sh`, which then reads `program` |
| `args` | Optional: up to 32 arguments of at most 1024 characters, after the program |

| Interpreter | Windows | macOS and Linux |
| --- | --- | --- |
| `python` | `py -3`, else `python` or `python3` on the search path | `python3`, else `python` |
| `powershell` | Windows PowerShell, which comes with Windows, with `-NoProfile -NonInteractive -ExecutionPolicy Bypass -File` | `pwsh` with `-NoProfile -NonInteractive -File` |
| `node` | `node` on the search path | `node` |
| `sh` | not available | `/bin/sh` |

Without an interpreter the program starts by itself: on Windows it must end
in `.exe`, `.com`, `.bat` or `.cmd`, and elsewhere it is made executable
when it is installed. PowerShell needs nothing to be installed on Windows,
which makes it a good choice there; Python is the common choice elsewhere.
When the interpreter is missing, the status bar says so when the user
starts the extension.

### uses

What the extension uses besides the window. The dialog that confirms the
install shows it to the user before anything runs.

| Field | What it is |
| --- | --- |
| `network` | `true` when it uses the internet or the network |
| `files_outside_folder` | `true` when it reads or writes files outside its own folder, such as a report the user saves or the scans themselves |
| `commands` | The commands of the local API it sends, as a list, or `"all"` |

The commands are enforced: the token a run gets is accepted for the commands
listed and refused with HTTP 403 for others. `show_message`,
`report_progress`, `context`, `choose_path` and `job` are always allowed.
`network` and `files_outside_folder` are what the extension declares; the
application cannot check them, see [Security](#security).

### contributes

`ribbon` holds up to 8 buttons. They appear in the **EXTENSIONS** group of
the ribbon, after SELECTION, which the ribbon shows while an enabled
extension has a button.

| Field | What it is |
| --- | --- |
| `id` | 1 to 64 lowercase letters and digits in parts joined by dots or dashes, unique within the extension |
| `label` | Text of at most 24 characters under the icon |
| `icon` | An `.svg` file in the folder of at most 64 KiB, shown at 32 by 32 pixels. It may not hold scripts, `<image>`, `<foreignObject>`, entities or style sheet imports, and links and `url()` may only point inside the image (`#id`) |
| `tooltip` | Optional text of at most 200 characters |
| `args` | Optional arguments added after those of the command |

`file_view` holds up to 4 tiles, with `id`, `page` (`new` or `export`),
`title` (at most 60 characters), an optional `detail` (at most 160) and
`args`. They appear under **EXTENSIONS** on that page of the File view.

## A run

A click on a button or a tile starts the program in the folder of the
extension, with the arguments of the command and then those of the button
or tile. It runs without a console window. An extension runs once at a time:
while it runs its buttons are highlighted, and a click on one of them stops
it.

The program gets these variables:

| Variable | What it holds |
| --- | --- |
| `OPS_API_PORT` | The port of the local API on `127.0.0.1` |
| `OPS_API_TOKEN` | The token of this run for the local API |
| `OPS_API_URL` | `http://127.0.0.1:<port>`, for convenience |
| `OPS_EXTENSION_ID` | The id of the extension |
| `OPS_CONTEXT` | The path of a JSON file with what the window showed when the run started |

`PYTHONUNBUFFERED` and `PYTHONIOENCODING` are set as well, so that the
output of Python comes in order and in UTF-8. The context file holds what
`context` reports, with the extension and the button or tile:

```json
{
  "application": {"version": "0.9.1", "language": "nl"},
  "active_scan": {
    "index": 0,
    "path": "D:\\scans\\hall.e57",
    "points": 48213977,
    "remaining": 48213977,
    "selected": 0,
    "bounds": {"min": [0.0, 0.0, -0.1], "max": [24.3, 18.9, 7.2]}
  },
  "scans": 1,
  "selected_points": 0,
  "section_box": {"min": [2.0, 3.0, 0.0], "max": [10.0, 9.0, 3.0], "rotation": 0.0},
  "shown": {"index": 0, "name": "3D model", "kind": "model", "active": true, "closable": false},
  "drawing_view": false,
  "file_view": false,
  "extension": {"id": "org.example.room-report", "version": "1.2.0", "folder": "C:\\Users\\…\\extensions\\org.example.room-report"},
  "entry": "report"
}
```

What the program writes to standard output and standard error goes to a log
file per run, `logs/run-<date>-<time>.log` in its folder (the time in UTC).
The log starts with the command line and ends with the exit code; it keeps
the first 4 MiB. The logs of the newest 20 runs are kept.

The status bar shows that the extension runs, with its progress and a
**Stop** button. When the run ends it says so: `<name> finished`, or
`<name> failed with exit code <n>` with the last lines of standard error, or
`<name> stopped`. A message the run showed with `show_message` stays when it
ends well. A run is stopped with its button, with **Stop** in the status bar
or on the Extensions page, with `stop_extension`, by switching the extension
off, and before it is updated or uninstalled. On Windows the program and
everything it started end at once; on macOS and Linux its process group gets
`SIGTERM` and, 1.5 seconds later, `SIGKILL`. When the window closes, every
run is ended.

## Using the local API

Each command is one `POST` of a JSON object to
`http://127.0.0.1:<OPS_API_PORT>/exec` with the token in the header
`X-OPS-Token`. The answer is a JSON object with `ok: true`, or `ok: false`
and an `error`. The server answers on this computer only: let the HTTP
client of the language leave out the proxy of the system. Send the body as
UTF-8.

These commands are made for extensions; [API.md](../native/API.md) describes
them and every other command:

| Command | What it does |
| --- | --- |
| `show_message` | `text`: a message in the status bar, after the name of the extension |
| `report_progress` | `percent` (0–100) and an optional `text`: the progress beside the name of the extension in the status bar |
| `context` | What the window shows now, as in the context file |
| `choose_path` | `mode` (`open`, `save` or `folder`), optional `title`, `filters`, `file_name`, `directory`: asks the user with a dialog of the window and answers with a `job_id`; `job` with that id reads `running` until the user answered, then `complete` with the `path`, or `cancelled` |
| `open` | `path`: opens a file the extension wrote, such as a point cloud or a mesh, as a layer |
| `status` | The state of the window: the open scans with their points, the camera, the section box and more |

### Python

```python
import json, os, urllib.request

URL = "http://127.0.0.1:{}/exec".format(os.environ["OPS_API_PORT"])
OPENER = urllib.request.build_opener(urllib.request.ProxyHandler({}))

def ops(command, **arguments):
    request = urllib.request.Request(
        URL,
        data=json.dumps(dict(arguments, command=command)).encode("utf-8"),
        headers={"X-OPS-Token": os.environ["OPS_API_TOKEN"], "Content-Type": "application/json"},
    )
    with OPENER.open(request, timeout=30) as response:
        answer = json.loads(response.read().decode("utf-8"))
    if not answer.get("ok"):
        raise RuntimeError(answer.get("error"))
    return answer

scans = ops("status")["result"]["clouds"]
ops("show_message", text="{} scans open".format(len(scans)))
```

### PowerShell

```powershell
$ErrorActionPreference = 'Stop'
$url = "http://127.0.0.1:$($env:OPS_API_PORT)/exec"
$headers = @{ 'X-OPS-Token' = $env:OPS_API_TOKEN }

function Invoke-Ops([hashtable]$command) {
    $body = [System.Text.Encoding]::UTF8.GetBytes((ConvertTo-Json -InputObject $command -Compress -Depth 8))
    $answer = Invoke-RestMethod -Uri $url -Method Post -Headers $headers `
        -Body $body -ContentType 'application/json; charset=utf-8'
    if (-not $answer.ok) { throw $answer.error }
    return $answer
}

$scans = @((Invoke-Ops @{ command = 'status' }).result.clouds)
Invoke-Ops @{ command = 'show_message'; text = "$($scans.Count) scans open" } | Out-Null
```

A refused command throws in both: the script ends with an error, and the
status bar shows the exit code and the last lines of the error.

## Installing, updating and removing

**Install extension…** on the Extensions page of the File view asks for a
`.zip` archive or the `extension.json` of a folder. The files are copied and
checked first; then a dialog shows the name, author, version, the command it
starts, what it uses and what it adds, and asks to confirm. `install_extension`
in the local API shows the same dialog; only the user confirms. An installed
extension is listed in `extensions.json` of the settings folder and kept
across restarts; a folder put into `extensions/` by hand is not loaded.

Installing a newer version, or the same one again, replaces the installed
one and keeps its logs and whether it is switched on. An older version is
refused; uninstall first to go back. **Uninstall…** removes the folder with
its logs after a confirmation. An installed extension that cannot be read
any more, because its folder is gone or its files were changed into
something that does not fit, is listed with the reason and can be
uninstalled.

## The example

[`native/extensions/examples/point-count-report`](../native/extensions/examples/point-count-report)
counts the points of the open scans. Its button, **Point count**, shows the
number of scans and points and the selection in the status bar; its tile on
the Export page, **Point count report…**, asks where to save a CSV file with
a line per scan and writes it. On Windows it runs `report.ps1` with Windows
PowerShell, elsewhere `report.py` with Python; both do the same. It reads
the context file, sends `status`, `report_progress`, `show_message`,
`choose_path` and `job`, and writes to its log. The tests of the application
install it in a temporary settings folder, run it against a window and check
its message, with PowerShell and with Python where they are installed.

## Security

An installed extension is a program that runs with the rights of the user,
not in a sandbox. It can read and change the files of the user and reach the
network, whatever it declares. The application therefore:

- installs nothing without the confirmation of the user, after showing what
  the extension declares, and runs nothing until the user starts it;
- copies only what passes the checks above: no links, no paths outside the
  folder, no archive entries that reach outside, limits on size and count,
  and icons that refer to nothing outside themselves;
- gives each run a token of its own that accepts only the commands the
  extension declared and stops working when the run ends;
- prefixes every message of an extension with its name, and names it in the
  title of the dialogs it asks for;
- keeps a log of every run, and ends every run when the window closes.

The declared commands hold for the token of the run; a hostile program could
still read the files of the user, the discovery file of the window among
them. Install extensions only from authors you trust, as you would any
program. As an author: declare what the extension uses honestly and keep the
list of commands short, write only where the user chose, keep no secrets in
the folder, and ask with `choose_path` rather than guess a path.

"""Point count report: an example extension of Open Pointcloud Studio.

It reads the status of the window through the local API and shows the
number of points of the open scans in the status bar. With --save, as the
tile on the Export page of the File view passes, it asks where to write a
CSV report with a line per scan. It speaks the language of the window,
English or Dutch, and groups digits with a point as the window does.

It needs Python 3.8 or newer and nothing beyond its standard library. The
window starts it with:
  OPS_API_PORT, OPS_API_TOKEN  the local API and the token of this run
  OPS_EXTENSION_ID             the id of the extension
  OPS_CONTEXT                  a JSON file with what the window shows
"""

import csv
import json
import os
import sys
import time
import traceback
import urllib.error
import urllib.request

URL = "http://127.0.0.1:{}/exec".format(os.environ["OPS_API_PORT"])
TOKEN = os.environ["OPS_API_TOKEN"]
# The API listens on this computer only; a proxy of the system is not asked.
OPENER = urllib.request.build_opener(urllib.request.ProxyHandler({}))

TEXTS = {
    "en": {
        "reading": "Reading the status",
        "counting": "Counting points",
        "scan": "scan",
        "scans": "scans",
        "summary": "{} {}, {} points, {} selected",
        "title": "Save the point count report",
        "table": "CSV table",
        "nothing": "No report was written",
        "written": "{}; report written to {}",
    },
    "nl": {
        "reading": "Status lezen",
        "counting": "Punten tellen",
        "scan": "scan",
        "scans": "scans",
        "summary": "{} {}, {} punten, {} geselecteerd",
        "title": "Puntentellingrapport opslaan",
        "table": "CSV-tabel",
        "nothing": "Er is geen rapport geschreven",
        "written": "{}; rapport opgeslagen in {}",
    },
}


def ops(command, **arguments):
    """Send one command and return its answer. A refusal raises with the
    reason: the window refuses with ok false, the server itself (an
    undeclared or unknown command) with HTTP 400, 403 or 504 and only an
    error."""
    body = json.dumps(dict(arguments, command=command)).encode("utf-8")
    request = urllib.request.Request(
        URL,
        data=body,
        headers={"X-OPS-Token": TOKEN, "Content-Type": "application/json"},
    )
    try:
        with OPENER.open(request, timeout=30) as response:
            answer = json.loads(response.read().decode("utf-8"))
    except urllib.error.HTTPError as error:
        try:
            answer = json.loads(error.read().decode("utf-8"))
        except ValueError:
            raise RuntimeError("{}: {}".format(command, error)) from None
    if not answer.get("ok"):
        raise RuntimeError("{}: {}".format(command, answer.get("error")))
    return answer


def count(number):
    """A count with its digits grouped by a point, as the window shows
    counts."""
    return "{:,}".format(number).replace(",", ".")


def main():
    save = "--save" in sys.argv[1:]
    # What the window showed when the run started, with its language.
    with open(os.environ["OPS_CONTEXT"], encoding="utf-8") as file:
        context = json.load(file)
    active = context.get("active_scan") or {}
    print("Started from: {}; active scan: {}".format(context.get("entry"), active.get("path")))
    language = (context.get("application") or {}).get("language")
    text = TEXTS.get(language, TEXTS["en"])

    ops("report_progress", percent=10, text=text["reading"])
    scans = ops("status")["result"]["clouds"]
    remaining = sum(scan["remaining"] for scan in scans)
    selected = sum(scan["selected"] for scan in scans)
    ops("report_progress", percent=60, text=text["counting"])

    noun = text["scan"] if len(scans) == 1 else text["scans"]
    summary = text["summary"].format(len(scans), noun, count(remaining), count(selected))
    print(summary)

    if not save:
        ops("show_message", text=summary)
        return

    # Ask where to write the report, and wait for the answer of the user.
    job = ops(
        "choose_path",
        mode="save",
        title=text["title"],
        file_name="point-count-report.csv",
        filters=[{"name": text["table"], "extensions": ["csv"]}],
    )["job_id"]
    while True:
        time.sleep(0.25)
        state = ops("job", id=job)["job"]
        if state["state"] != "running":
            break
    if state["state"] != "complete":
        ops("show_message", text=text["nothing"])
        return

    with open(state["path"], "w", newline="", encoding="utf-8") as file:
        writer = csv.writer(file)
        writer.writerow(["scan", "points", "remaining", "selected"])
        for scan in scans:
            writer.writerow([scan["path"], scan["points"], scan["remaining"], scan["selected"]])
    ops("show_message", text=text["written"].format(summary, state["path"]))


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        # Every failure ends the run with only its reason on standard error,
        # which the status bar shows; where it happened goes to the log.
        traceback.print_exc(file=sys.stdout)
        sys.stdout.flush()
        print(error, file=sys.stderr)
        sys.exit(1)

"""Point count report: an example extension of Open Pointcloud Studio.

It reads the status of the window through the local API and shows the
number of points of the open scans in the status bar. With --save, as the
tile on the Export page of the File view passes, it asks where to write a
CSV report with a line per scan.

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
import urllib.request

URL = "http://127.0.0.1:{}/exec".format(os.environ["OPS_API_PORT"])
TOKEN = os.environ["OPS_API_TOKEN"]
# The API listens on this computer only; a proxy of the system is not asked.
OPENER = urllib.request.build_opener(urllib.request.ProxyHandler({}))


def ops(command, **arguments):
    """Send one command and return its answer; a refusal ends the script
    with the reason, which the status bar shows."""
    body = json.dumps(dict(arguments, command=command)).encode("utf-8")
    request = urllib.request.Request(
        URL,
        data=body,
        headers={"X-OPS-Token": TOKEN, "Content-Type": "application/json"},
    )
    with OPENER.open(request, timeout=30) as response:
        answer = json.loads(response.read().decode("utf-8"))
    if not answer.get("ok"):
        raise RuntimeError("{}: {}".format(command, answer.get("error")))
    return answer


def main():
    save = "--save" in sys.argv[1:]
    # What the window showed when the run started.
    with open(os.environ["OPS_CONTEXT"], encoding="utf-8") as file:
        context = json.load(file)
    active = context.get("active_scan") or {}
    print("Started from: {}; active scan: {}".format(context.get("entry"), active.get("path")))

    ops("report_progress", percent=10, text="Reading the status")
    scans = ops("status")["result"]["clouds"]
    remaining = sum(scan["remaining"] for scan in scans)
    selected = sum(scan["selected"] for scan in scans)
    ops("report_progress", percent=60, text="Counting points")

    noun = "scan" if len(scans) == 1 else "scans"
    summary = "{} {}, {:,} points, {:,} selected".format(len(scans), noun, remaining, selected)
    print(summary)

    if not save:
        ops("show_message", text=summary)
        return

    # Ask where to write the report, and wait for the answer of the user.
    job = ops(
        "choose_path",
        mode="save",
        title="Save the point count report",
        file_name="point-count-report.csv",
        filters=[{"name": "CSV table", "extensions": ["csv"]}],
    )["job_id"]
    while True:
        time.sleep(0.25)
        state = ops("job", id=job)["job"]
        if state["state"] != "running":
            break
    if state["state"] != "complete":
        ops("show_message", text="No report was written")
        return

    with open(state["path"], "w", newline="", encoding="utf-8") as file:
        writer = csv.writer(file)
        writer.writerow(["scan", "points", "remaining", "selected"])
        for scan in scans:
            writer.writerow([scan["path"], scan["points"], scan["remaining"], scan["selected"]])
    ops("show_message", text="{}; report written to {}".format(summary, state["path"]))


if __name__ == "__main__":
    main()

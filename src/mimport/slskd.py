from __future__ import annotations

import json
import logging
import time
from pathlib import Path
from typing import Any, Callable
from urllib.parse import quote

from .errors import MimportError
from .http import SlskdClient, _SlskdNotFound

__all__ = [
    "SlskdClient",
    "search",
    "list_searches",
    "search_status",
    "view_search",
    "search_remove",
    "enqueue_downloads",
    "list_downloads",
    "remove_download",
    "clear_completed_downloads",
    "split_remote_path",
    "resolve_selector",
    "transfer_status",
    "cancel_transfer",
    "browse_directory",
    "batch_timeout",
    "is_terminal_state",
    "fetch_and_wait",
    "wait_for_transfers",
    "clear_transfers",
    "fetch_into_job",
    "run",
]

logger = logging.getLogger(__name__)

SEARCH_REQUEST_TIMEOUT = 600.0
POLL_INTERVAL = 1.5
LOSSLESS_EXTENSIONS = ("flac", "wav", "ape", "wv", "aiff", "alac")
DEFAULT_TOP_RESPONSES = 5
CANCEL_SETTLE_POLLS = 10
CANCEL_POLL_INTERVAL = 1.0


def _urlencode(value: str) -> str:
    return quote(value, safe="-_.~")


def _obj(value: Any, what: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise MimportError(f"json error: expected object for {what}")
    return value


def _req_str(data: dict[str, Any], key: str) -> str:
    value = data.get(key)
    if not isinstance(value, str):
        raise MimportError(f"json error: missing field `{key}`")
    return value


def _opt_str(data: dict[str, Any], key: str) -> str | None:
    value = data.get(key)
    if value is None:
        return None
    if not isinstance(value, str):
        raise MimportError(f"json error: invalid type for field `{key}`")
    return value


def _req_int(data: dict[str, Any], key: str) -> int:
    value = data.get(key)
    if isinstance(value, bool) or not isinstance(value, int):
        raise MimportError(f"json error: missing field `{key}`")
    return int(value)


def _opt_int(data: dict[str, Any], key: str) -> int | None:
    value = data.get(key)
    if value is None:
        return None
    if isinstance(value, bool) or not isinstance(value, int):
        raise MimportError(f"json error: invalid type for field `{key}`")
    return int(value)


def _def_int(data: dict[str, Any], key: str) -> int:
    value = data.get(key)
    if value is None:
        return 0
    if isinstance(value, bool) or not isinstance(value, int):
        raise MimportError(f"json error: invalid type for field `{key}`")
    return int(value)


def _req_bool(data: dict[str, Any], key: str) -> bool:
    value = data.get(key)
    if not isinstance(value, bool):
        raise MimportError(f"json error: missing field `{key}`")
    return value


def _def_bool(data: dict[str, Any], key: str) -> bool:
    value = data.get(key)
    if value is None:
        return False
    if not isinstance(value, bool):
        raise MimportError(f"json error: invalid type for field `{key}`")
    return value


def _def_float(data: dict[str, Any], key: str) -> float:
    value = data.get(key)
    if value is None:
        return 0.0
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise MimportError(f"json error: invalid type for field `{key}`")
    return float(value)


def _def_list(data: dict[str, Any], key: str) -> list[Any]:
    value = data.get(key)
    if value is None:
        return []
    if not isinstance(value, list):
        raise MimportError(f"json error: invalid type for field `{key}`")
    return value


def _normalize_file(raw: Any) -> dict[str, Any]:
    data = _obj(raw, "file")
    return {
        "filename": _req_str(data, "filename"),
        "size": _req_int(data, "size"),
        "extension": _opt_str(data, "extension"),
        "bitRate": _opt_int(data, "bitRate"),
        "bitDepth": _opt_int(data, "bitDepth"),
        "sampleRate": _opt_int(data, "sampleRate"),
        "length": _opt_int(data, "length"),
        "isVariableBitRate": _def_bool(data, "isVariableBitRate"),
        "isLocked": _def_bool(data, "isLocked"),
    }


def _normalize_search_response(raw: Any) -> dict[str, Any]:
    data = _obj(raw, "search response")
    return {
        "username": _req_str(data, "username"),
        "token": _req_int(data, "token"),
        "fileCount": _def_int(data, "fileCount"),
        "files": [_normalize_file(item) for item in _def_list(data, "files")],
        "lockedFileCount": _def_int(data, "lockedFileCount"),
        "lockedFiles": [
            _normalize_file(item) for item in _def_list(data, "lockedFiles")
        ],
        "hasFreeUploadSlot": _req_bool(data, "hasFreeUploadSlot"),
        "queueLength": _req_int(data, "queueLength"),
        "uploadSpeed": _req_int(data, "uploadSpeed"),
    }


def _normalize_search(raw: Any) -> dict[str, Any]:
    data = _obj(raw, "search")
    return {
        "id": _req_str(data, "id"),
        "token": _req_int(data, "token"),
        "searchText": _req_str(data, "searchText"),
        "state": _req_str(data, "state"),
        "isComplete": _req_bool(data, "isComplete"),
        "fileCount": _def_int(data, "fileCount"),
        "lockedFileCount": _def_int(data, "lockedFileCount"),
        "responseCount": _def_int(data, "responseCount"),
        "responses": [
            _normalize_search_response(item)
            for item in _def_list(data, "responses")
        ],
        "startedAt": _opt_str(data, "startedAt"),
        "endedAt": _opt_str(data, "endedAt"),
    }


def _normalize_transfer(raw: Any) -> dict[str, Any]:
    data = _obj(raw, "transfer")
    return {
        "id": _req_str(data, "id"),
        "username": _req_str(data, "username"),
        "filename": _req_str(data, "filename"),
        "direction": _req_str(data, "direction"),
        "state": _req_str(data, "state"),
        "size": _req_int(data, "size"),
        "bytesTransferred": _def_int(data, "bytesTransferred"),
        "bytesRemaining": _def_int(data, "bytesRemaining"),
        "percentComplete": _def_float(data, "percentComplete"),
        "averageSpeed": _def_float(data, "averageSpeed"),
        "elapsedTime": _opt_str(data, "elapsedTime"),
        "remainingTime": _opt_str(data, "remainingTime"),
        "placeInQueue": _opt_int(data, "placeInQueue"),
        "exception": _opt_str(data, "exception"),
    }


def _normalize_enqueue_result(raw: Any) -> dict[str, Any]:
    data = _obj(raw, "enqueue result")
    failed = data.get("failed")
    if failed is None:
        failed_items: list[Any] = []
    elif isinstance(failed, list):
        failed_items = list(failed)
    else:
        raise MimportError("json error: invalid type for field `failed`")
    return {
        "enqueued": [
            _normalize_transfer(item) for item in _def_list(data, "enqueued")
        ],
        "failed": failed_items,
    }


def _normalize_directory(raw: Any) -> dict[str, Any]:
    data = _obj(raw, "directory")
    return {
        "name": _req_str(data, "name"),
        "files": [_normalize_file(item) for item in _def_list(data, "files")],
        "directories": [
            _normalize_directory(item) for item in _def_list(data, "directories")
        ],
    }


def _normalize_directory_transfers(raw: Any) -> dict[str, Any]:
    data = _obj(raw, "directory transfers")
    return {
        "directory": _req_str(data, "directory"),
        "fileCount": _def_int(data, "fileCount"),
        "files": [
            _normalize_transfer(item) for item in _def_list(data, "files")
        ],
    }


def _normalize_downloads_response(raw: Any) -> dict[str, Any]:
    data = _obj(raw, "downloads response")
    return {
        "username": _req_str(data, "username"),
        "directories": [
            _normalize_directory_transfers(item)
            for item in _def_list(data, "directories")
        ],
    }


def search(client: SlskdClient, query: str, fresh: bool) -> dict[str, Any]:
    if not fresh:
        existing = reusable_search_id(client, query)
        if existing is not None:
            return {"search": search_status(client, existing), "reused": True}
    return run_search(client, query)


def run_search(client: SlskdClient, query: str) -> dict[str, Any]:
    submitted = _normalize_search(
        client.post_with_timeout(
            "/api/v0/searches",
            {"searchText": query},
            SEARCH_REQUEST_TIMEOUT,
        )
    )

    current = submitted
    deadline = time.monotonic() + SEARCH_REQUEST_TIMEOUT
    while not current["isComplete"]:
        if time.monotonic() >= deadline:
            raise MimportError(
                "slskd search failed: HTTP 0\nsearch never completed"
            )
        time.sleep(POLL_INTERVAL)
        current = search_status(client, current["id"])

    attempts = 0
    while not current["responses"] and attempts < 5:
        time.sleep(0.5)
        current = search_status(client, current["id"])
        attempts += 1

    return {"search": current, "reused": False}


def _ended_or_started(item: dict[str, Any]) -> str:
    if item["endedAt"] is not None:
        return item["endedAt"]
    if item["startedAt"] is not None:
        return item["startedAt"]
    return ""


def reusable_search_id(client: SlskdClient, query: str) -> str | None:
    candidates = [
        item
        for item in list_searches(client)
        if item["searchText"] == query
        and item["isComplete"]
        and item["fileCount"] > 0
    ]
    if not candidates:
        return None
    best = max(candidates, key=lambda item: (_ended_or_started(item), item["id"]))
    return best["id"]


def list_searches(client: SlskdClient) -> list[dict[str, Any]]:
    response = client.get("/api/v0/searches")
    if not isinstance(response, list):
        raise MimportError("json error: expected array for searches")
    return [_normalize_search(item) for item in response]


def search_status(client: SlskdClient, search_id: str) -> dict[str, Any]:
    return _normalize_search(
        client.get(f"/api/v0/searches/{_urlencode(search_id)}?includeResponses=true")
    )


def is_lossless_file(file: dict[str, Any]) -> bool:
    extension = file["extension"]
    if extension is not None and extension.lower() in LOSSLESS_EXTENSIONS:
        return True
    return file["bitDepth"] is not None and file["sampleRate"] is not None


def view_search(search: dict[str, Any], opts: dict[str, Any]) -> dict[str, Any]:
    lossless_only = bool(opts.get("lossless_only", False))
    show_all = bool(opts.get("all", False))

    responses: list[dict[str, Any]] = []
    for response in search["responses"]:
        if lossless_only:
            files = [f for f in response["files"] if is_lossless_file(f)]
            if not files:
                continue
            locked_files = [
                f for f in response["lockedFiles"] if is_lossless_file(f)
            ]
        else:
            files = list(response["files"])
            locked_files = list(response["lockedFiles"])
        viewed = dict(response)
        viewed["files"] = files
        viewed["lockedFiles"] = locked_files
        viewed["fileCount"] = len(files)
        viewed["lockedFileCount"] = len(locked_files)
        responses.append(viewed)

    responses.sort(
        key=lambda item: (
            not item["hasFreeUploadSlot"],
            -item["uploadSpeed"],
            item["queueLength"],
            -item["fileCount"],
            item["username"],
        )
    )
    if not show_all:
        del responses[DEFAULT_TOP_RESPONSES:]

    viewed_search = dict(search)
    viewed_search["responses"] = responses
    viewed_search["fileCount"] = sum(item["fileCount"] for item in responses)
    viewed_search["responseCount"] = len(responses)
    return viewed_search


def search_remove(client: SlskdClient, search_id: str) -> None:
    client.delete(f"/api/v0/searches/{_urlencode(search_id)}")


def _request_items(items: list[dict[str, Any]]) -> list[dict[str, Any]]:
    return [
        {"filename": _req_str(item, "filename"), "size": _req_int(item, "size")}
        for item in items
    ]


def enqueue_downloads(
    client: SlskdClient,
    username: str,
    items: list[dict[str, Any]],
) -> dict[str, Any]:
    path = f"/api/v0/transfers/downloads/{_urlencode(username)}"
    return _normalize_enqueue_result(client.post(path, _request_items(items)))


def list_downloads(client: SlskdClient) -> list[dict[str, Any]]:
    response = client.get("/api/v0/transfers/downloads")
    if not isinstance(response, list):
        raise MimportError("json error: expected array for downloads")
    return [_normalize_downloads_response(item) for item in response]


def remove_download(client: SlskdClient, username: str, transfer_id: str) -> None:
    path = (
        f"/api/v0/transfers/downloads/{_urlencode(username)}/"
        f"{_urlencode(transfer_id)}?remove=true"
    )
    try:
        client.delete(path)
    except _SlskdNotFound:
        return


def clear_completed_downloads(client: SlskdClient) -> None:
    client.delete("/api/v0/transfers/downloads/all/completed")


def split_remote_path(path: str) -> tuple[str, str]:
    index = path.rfind("\\")
    if index < 0:
        return ("", path)
    return (path[:index], path[index + 1 :])


def resolve_selector(
    search: dict[str, Any],
    username: str,
    directory: str,
    filenames: list[str],
) -> list[dict[str, Any]]:
    response = next(
        (item for item in search["responses"] if item["username"] == username),
        None,
    )
    if response is None:
        raise MimportError(
            "slskd fetch selector matched no peer: "
            f"no response from {username} in search {search['id']}"
        )

    in_dir = [
        file
        for file in response["files"]
        if split_remote_path(file["filename"])[0] == directory
    ]
    if not in_dir:
        raise MimportError(
            "slskd fetch selector matched no directory: "
            f"{username} has no files under "
            f"{json.dumps(directory, ensure_ascii=False)}"
        )

    if not filenames:
        return in_dir

    matched: list[dict[str, Any]] = []
    seen: set[str] = set()
    missing: list[str] = []
    for name in filenames:
        if name in seen:
            continue
        seen.add(name)
        found = next(
            (
                file
                for file in in_dir
                if split_remote_path(file["filename"])[1] == name
            ),
            None,
        )
        if found is None:
            missing.append(name)
        else:
            matched.append(found)
    if missing:
        raise MimportError(
            "slskd fetch selector matched no file: "
            f"{username}: {directory}\\ (missing: {', '.join(missing)})"
        )
    return matched


def transfer_status(
    client: SlskdClient, username: str, transfer_id: str
) -> dict[str, Any]:
    return _normalize_transfer(
        client.get(
            f"/api/v0/transfers/downloads/{_urlencode(username)}/"
            f"{_urlencode(transfer_id)}"
        )
    )


def cancel_transfer(
    client: SlskdClient,
    username: str,
    transfer_id: str,
    remove: bool,
) -> None:
    flag = "true" if remove else "false"
    client.delete(
        f"/api/v0/transfers/downloads/{_urlencode(username)}/"
        f"{_urlencode(transfer_id)}?remove={flag}"
    )


def browse_directory(
    client: SlskdClient, username: str, directory: str
) -> list[dict[str, Any]]:
    response = client.post(
        f"/api/v0/users/{_urlencode(username)}/directory",
        {"directory": directory},
    )
    if not isinstance(response, list):
        raise MimportError("json error: expected array for directory browse")
    return [_normalize_directory(item) for item in response]


def batch_timeout(
    cfg: Any,
    items: list[dict[str, Any]],
    override_secs: int | None,
) -> float:
    if override_secs is not None:
        return float(max(int(override_secs), 1))
    total_mb = sum(max(int(item["size"]), 0) for item in items) / (1024.0 * 1024.0)
    secs = (
        float(cfg.fetch_timeout_base_secs)
        + total_mb * float(cfg.fetch_timeout_per_mb_secs)
        + len(items) * float(cfg.fetch_timeout_per_file_secs)
    )
    return max(secs, 1.0)


def is_terminal_state(state: str) -> bool:
    return (
        state.startswith("Completed")
        or "Cancelled" in state
        or "Errored" in state
        or "Rejected" in state
    )


def fetch_and_wait(
    client: SlskdClient,
    username: str,
    window: float,
    items: list[dict[str, Any]],
    on_update: Callable[[dict[str, Any]], Any],
) -> list[dict[str, Any]]:
    enqueued = enqueue_downloads(client, username, items)
    if not enqueued["enqueued"]:
        raise MimportError(
            "slskd enqueue failed: HTTP 0\n"
            "no transfers returned in enqueued[] "
            f"(failed: {_debug_list(enqueued['failed'])})"
        )
    return wait_for_transfers(
        client, username, enqueued["enqueued"], window, on_update
    )


def _signature(transfer: dict[str, Any]) -> tuple[str, int, int | None]:
    return (
        transfer["state"],
        transfer["bytesTransferred"],
        transfer["placeInQueue"],
    )


def wait_for_transfers(
    client: SlskdClient,
    username: str,
    transfers: list[dict[str, Any]],
    window: float,
    on_update: Callable[[dict[str, Any]], Any],
) -> list[dict[str, Any]]:
    pending: list[dict[str, Any]] = []
    for transfer in transfers:
        pending.append(
            {
                "transfer": transfer,
                "sig": _signature(transfer),
                "deadline": time.monotonic() + window,
                "window": window,
            }
        )

    results: list[dict[str, Any]] = []
    timeout_err: MimportError | None = None

    while pending:
        index = 0
        while index < len(pending):
            item = pending[index]
            if time.monotonic() >= item["deadline"]:
                error = MimportError(
                    f"slskd transfer {username}/{item['transfer']['id']} timed out "
                    f"waiting for completion after {int(item['window'])}s "
                    f"(last state: {item['transfer']['state']})"
                )
                if timeout_err is None:
                    timeout_err = error
                del pending[index]
                continue

            transfer_id = item["transfer"]["id"]
            try:
                transfer = transfer_status(client, username, transfer_id)
            except MimportError as exc:
                if time.monotonic() >= item["deadline"]:
                    if timeout_err is None:
                        timeout_err = exc
                    del pending[index]
                    continue
            else:
                on_update(transfer)
                if is_terminal_state(transfer["state"]):
                    results.append(transfer)
                    del pending[index]
                    continue
                signature = _signature(transfer)
                if signature != item["sig"]:
                    item["sig"] = signature
                    item["deadline"] = time.monotonic() + item["window"]
                item["transfer"] = transfer
            index += 1

        if pending:
            time.sleep(POLL_INTERVAL)

    if timeout_err is not None:
        raise timeout_err
    return results


def clear_transfers(
    client: SlskdClient, username: str, old_ids: list[str]
) -> None:
    for transfer_id in old_ids:
        try:
            cancel_transfer(client, username, transfer_id, False)
        except _SlskdNotFound:
            pass

        deadline = time.monotonic() + 5.0
        while True:
            try:
                transfer = transfer_status(client, username, transfer_id)
            except _SlskdNotFound:
                break
            if is_terminal_state(transfer["state"]):
                break
            if time.monotonic() >= deadline:
                break
            time.sleep(CANCEL_POLL_INTERVAL / 2)

        try:
            cancel_transfer(client, username, transfer_id, True)
        except _SlskdNotFound:
            pass

    attempts = 0
    while attempts < CANCEL_SETTLE_POLLS:
        still_present = any(
            download["username"] == username
            and any(
                any(
                    file["id"] in old_ids
                    for file in directory["files"]
                )
                for directory in download["directories"]
            )
            for download in list_downloads(client)
        )
        if not still_present:
            return
        time.sleep(CANCEL_POLL_INTERVAL)
        attempts += 1
    logger.warning(
        "slskd still lists some of the removed transfers; re-enqueue may be rejected"
    )


def fetch_into_job(
    client: SlskdClient,
    conn: Any,
    job_id: int,
    username: str,
    local_dir: Path,
    files: list[dict[str, Any]],
    window: float,
) -> None:
    from . import jobs

    def on_update(transfer: dict[str, Any]) -> None:
        jobs.upsert_job_file(conn, job_id, local_dir, transfer)

    transfers = fetch_and_wait(client, username, window, files, on_update)
    jobs.set_job_status(conn, job_id, jobs.derive_status(transfers))


def _debug_list(values: list[Any]) -> str:
    return "[" + ", ".join(_debug_value(item) for item in values) + "]"


def _debug_value(value: Any) -> str:
    if value is None:
        return "Null"
    if isinstance(value, bool):
        return f"Bool({'true' if value else 'false'})"
    if isinstance(value, (int, float)):
        return f"Number({value})"
    if isinstance(value, str):
        return f"String({json.dumps(value, ensure_ascii=False)})"
    if isinstance(value, list):
        return "Array [" + ", ".join(_debug_value(item) for item in value) + "]"
    if isinstance(value, dict):
        entries = ", ".join(
            f"{json.dumps(str(key), ensure_ascii=False)}: {_debug_value(item)}"
            for key, item in value.items()
        )
        return "Object {" + entries + "}"
    return repr(value)


def run(cfg: Any, args: Any) -> Any:
    from . import jobs

    action = str(args.action)

    with SlskdClient(cfg.slskd) as client:
        if action == "search":
            outcome = search(
                client, str(args.query), bool(getattr(args, "fresh", False))
            )
            view = view_search(
                outcome["search"],
                {
                    "lossless_only": True,
                    "all": bool(getattr(args, "all", False)),
                },
            )
            return {"reused": outcome["reused"], "search": view}

        if action == "searches":
            return list_searches(client)

        if action == "search-status":
            status = search_status(client, str(args.id))
            return view_search(
                status,
                {
                    "lossless_only": True,
                    "all": bool(getattr(args, "all", False)),
                },
            )

        if action == "search-remove":
            search_id = str(args.id)
            search_remove(client, search_id)
            return {"removed": search_id}

        if action == "fetch":
            search_id = str(args.search_id)
            username = str(args.username)
            directory = str(args.directory)
            filenames = [str(name) for name in getattr(args, "filenames", None) or []]
            search_obj = search_status(client, search_id)
            selected = resolve_selector(search_obj, username, directory, filenames)
            items = [
                {"filename": file["filename"], "size": file["size"]}
                for file in selected
            ]
            window = batch_timeout(
                cfg.slskd, items, getattr(args, "wait_secs", None)
            )

            db = jobs.open(cfg.paths.database)
            try:
                local_dir = jobs.local_dir_for(
                    cfg.paths.downloads, username, directory
                )
                title = getattr(args, "title", None)
                job_title = (
                    str(title)
                    if title is not None
                    else jobs.default_title(directory)
                )
                job_id = jobs.create_job(
                    db,
                    job_title,
                    search_id,
                    username,
                    directory,
                    local_dir,
                )
                fetch_into_job(
                    client,
                    db,
                    job_id,
                    username,
                    local_dir,
                    items,
                    window,
                )
                job = jobs.get_job(db, job_id)
                job_files = jobs.get_job_files(db, job_id)
                return {"job": job, "files": job_files}
            finally:
                db.close()

        if action == "status":
            db = jobs.open(cfg.paths.database)
            try:
                job = jobs.resolve_target(db, str(args.target))
                job_files = jobs.get_job_files(db, job["id"])
                return {"job": job, "files": job_files}
            finally:
                db.close()

        if action == "cancel":
            db = jobs.open(cfg.paths.database)
            try:
                job = jobs.resolve_target(db, str(args.target))
                job_files = jobs.get_job_files(db, job["id"])
                cancelled_any = False
                for file in job_files:
                    if is_terminal_state(file["state"]):
                        continue
                    cancelled_any = True
                    cancel_transfer(
                        client,
                        job["username"],
                        file["transfer_id"],
                        bool(getattr(args, "remove", False)),
                    )
                    try:
                        transfer = transfer_status(
                            client, job["username"], file["transfer_id"]
                        )
                    except _SlskdNotFound:
                        jobs.set_job_file_state(
                            db, job["id"], file["transfer_id"], "Cancelled"
                        )
                    else:
                        jobs.upsert_job_file(
                            db,
                            job["id"],
                            Path(job["local_dir"]),
                            transfer,
                        )
                job_files = jobs.get_job_files(db, job["id"])
                if cancelled_any:
                    states = [file["state"] for file in job_files]
                    jobs.set_job_status(
                        db,
                        job["id"],
                        jobs.derive_status_from_states(states),
                    )
                job = jobs.get_job(db, job["id"])
                return {"job": job, "files": job_files}
            finally:
                db.close()

        if action == "retry":
            db = jobs.open(cfg.paths.database)
            try:
                job = jobs.resolve_target(db, str(args.target))
                job_files = jobs.get_job_files(db, job["id"])
                retryable = jobs.retryable_files(job_files)
                if not retryable:
                    return {
                        "job": job,
                        "retried": 0,
                        "files": job_files,
                    }

                dropped_ids = [file["transfer_id"] for file in retryable]
                items = [
                    {"filename": file["remote_path"], "size": file["size"]}
                    for file in retryable
                ]

                clear_transfers(client, job["username"], dropped_ids)
                enqueued = enqueue_downloads(client, job["username"], items)
                if not enqueued["enqueued"]:
                    raise MimportError(
                        "slskd enqueue failed: HTTP 0\n"
                        "no transfers returned in enqueued[] "
                        f"(failed: {_debug_list(enqueued['failed'])})"
                    )
                jobs.delete_job_files(db, job["id"], dropped_ids)
                jobs.set_job_status(db, job["id"], "incomplete")

                local_dir = Path(job["local_dir"])
                window = batch_timeout(
                    cfg.slskd, items, getattr(args, "wait_secs", None)
                )

                def on_update(transfer: dict[str, Any]) -> None:
                    jobs.upsert_job_file(db, job["id"], local_dir, transfer)

                transfers = wait_for_transfers(
                    client,
                    job["username"],
                    enqueued["enqueued"],
                    window,
                    on_update,
                )
                jobs.set_job_status(db, job["id"], jobs.derive_status(transfers))
                job = jobs.get_job(db, job["id"])
                job_files = jobs.get_job_files(db, job["id"])
                return {"job": job, "files": job_files}
            finally:
                db.close()

        if action == "downloads":
            return list_downloads(client)

        if action == "remove":
            username = str(args.username)
            transfer_id = str(args.transfer_id)
            remove_download(client, username, transfer_id)
            return {
                "username": username,
                "transfer_id": transfer_id,
                "removed": True,
            }

        if action == "clear-completed":
            clear_completed_downloads(client)
            return {"cleared": True}

        if action == "browse":
            username = str(args.username)
            directory = str(getattr(args, "directory", "") or "")
            return browse_directory(client, username, directory)

    raise MimportError(f"unknown slskd action: {action}")

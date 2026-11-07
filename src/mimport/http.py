from __future__ import annotations

import json
import re
import time
from typing import Any
from urllib.parse import quote

import httpx

from .cache import DiskCache
from .errors import MimportError, ServiceNotFound

MAX_RETRY_AFTER_SECS = 60
MB_TIMEOUT_SECS = 30.0
LIDARR_TIMEOUT_SECS = 15.0
LIDARR_MIN_INTERVAL_SECS = 0.35
LIDARR_USER_AGENT = "mimport"
RETRYABLE_MB = frozenset({429, 502, 503, 504})
RETRYABLE_LIDARR = frozenset({429, 500, 502, 503, 504})

_MISSING = object()
_SENSITIVE_KEYS = frozenset(
    {
        "token",
        "password",
        "authorization",
        "auth",
        "access_token",
        "refresh_token",
        "bearer",
        "secret",
        "api_key",
        "apikey",
        "username",
    }
)
_URL_USERINFO_RE = re.compile(r"(?i)([a-z][a-z0-9+.-]*://)([^/@\s]+)@")
_BEARER_RE = re.compile(r"(?i)(bearer\s+)[^\s,;\"'}]+")
_KEY_VALUE_RE = re.compile(
    r"(?i)((?:\"?)(?:token|password|authorization|auth|access_token|refresh_token|secret|api_key|apikey|username)(?:\"?)\s*[:=]\s*)(\"[^\"]*\"|[^\s,;}]+)"
)
_CONFIG_ERROR_TYPES = (
    httpx.InvalidURL,
    httpx.LocalProtocolError,
    httpx.UnsupportedProtocol,
    ValueError,
    TypeError,
)
_CLIENT_BUILD_ERRORS = _CONFIG_ERROR_TYPES + (httpx.HTTPError,)


class _SlskdNotFound(ServiceNotFound):
    pass


def _urlencode(value: str) -> str:
    return quote(value, safe="-_.~")


def _secret_variants(secrets: tuple[str, ...]) -> tuple[str, ...]:
    variants: set[str] = set()
    for secret in secrets:
        if not secret:
            continue
        variants.add(secret)
        encoded = quote(secret, safe="")
        variants.add(encoded)
        variants.add(encoded.replace("%20", "+"))
        variants.add(json.dumps(secret)[1:-1])
    return tuple(sorted(variants, key=len, reverse=True))


def _redact_text(text: str, secrets: tuple[str, ...] = ()) -> str:
    redacted = _URL_USERINFO_RE.sub(r"\1[redacted]@", text)
    redacted = _BEARER_RE.sub(r"\1[redacted]", redacted)
    redacted = _KEY_VALUE_RE.sub(r"\1[redacted]", redacted)
    for secret in _secret_variants(secrets):
        redacted = redacted.replace(secret, "[redacted]")
    return redacted


def _redact_value(value: Any, secrets: tuple[str, ...]) -> Any:
    if isinstance(value, dict):
        return {
            (
                _redact_text(key, secrets) if isinstance(key, str) else key
            ): (
                "[redacted]"
                if isinstance(key, str) and key.lower() in _SENSITIVE_KEYS
                else _redact_value(item, secrets)
            )
            for key, item in value.items()
        }
    if isinstance(value, list):
        return [_redact_value(item, secrets) for item in value]
    if isinstance(value, str):
        return _redact_text(value, secrets)
    return value


def _redact_body(text: str, secrets: tuple[str, ...] = ()) -> str:
    try:
        parsed = json.loads(text)
    except (json.JSONDecodeError, TypeError):
        return _redact_text(text, secrets)
    try:
        return json.dumps(_redact_value(parsed, secrets), ensure_ascii=False)
    except (TypeError, ValueError):
        return _redact_text(text, secrets)


def _config_error(exc: BaseException, secrets: tuple[str, ...] = ()) -> MimportError:
    return MimportError(
        f"http client configuration error: {_redact_text(str(exc), secrets)}"
    )


def _transport_error(exc: BaseException, secrets: tuple[str, ...] = ()) -> MimportError:
    return MimportError(f"http transport error: {_redact_text(str(exc), secrets)}")


def _parse_json(text: str) -> Any:
    try:
        return json.loads(text)
    except json.JSONDecodeError as exc:
        raise MimportError(f"json error: {exc}") from exc


def _backoff(attempt: int) -> float:
    return 0.5 * (2 ** min(attempt, 6))


def _retry_after(response: httpx.Response) -> float | None:
    raw = response.headers.get("retry-after")
    if raw is None:
        return None
    try:
        secs = int(raw)
    except (TypeError, ValueError):
        return None
    if secs < 0:
        return None
    return float(min(secs, MAX_RETRY_AFTER_SECS))


def _mb_not_found(path: str) -> MimportError:
    segments = path.lstrip("/").split("/")
    head = segments[0] if segments else ""
    entity = head if head in ("artist", "release", "release-group", "recording") else "entity"
    mbid = segments[1] if len(segments) > 1 else ""
    return ServiceNotFound(
        f"musicbrainz has no {entity} with mbid {mbid}",
        service="musicbrainz",
        status=404,
        resource=mbid,
    )


def _lidarr_not_found(path: str) -> MimportError:
    segments = path.lstrip("/").split("/")
    head = segments[0] if segments else ""
    entity = head if head in ("artist", "album") else "entity"
    mbid = segments[1] if len(segments) > 1 else ""
    return ServiceNotFound(
        f"lidarr proxy has no {entity} with mbid {mbid}",
        service="lidarr",
        status=404,
        resource=mbid,
    )


class MbClient:
    def __init__(self, cfg: Any) -> None:
        try:
            self._http = httpx.Client(
                timeout=MB_TIMEOUT_SECS, follow_redirects=True, max_redirects=10
            )
            self._base_url = str(cfg.base_url).rstrip("/")
            self._user_agent = str(cfg.user_agent)
            self._cache = DiskCache(cfg.cache_dir, int(cfg.cache_ttl_secs))
            self._max_retries = int(cfg.max_retries)
            self._min_interval = 1.0 / max(float(cfg.rate_limit_per_sec), 0.01)
            self._last = time.monotonic() - self._min_interval
        except _CLIENT_BUILD_ERRORS as exc:
            raise _config_error(exc) from exc

    def __enter__(self) -> MbClient:
        return self

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> bool:
        self.close()
        return False

    def close(self) -> None:
        self._http.close()

    def get(self, path: str, params: list[tuple[str, str]] | None = None) -> Any:
        url = f"{self._base_url}{path}?fmt=json"
        for key, value in params or []:
            url += f"&{key}={_urlencode(value)}"

        cached = self._cache.get(url)
        if cached is not None:
            return _parse_json(cached)

        body = self._fetch_with_retry(path, url)
        value = _parse_json(body)
        self._cache.put(url, body)
        return value

    def _wait(self) -> None:
        elapsed = time.monotonic() - self._last
        if elapsed < self._min_interval:
            time.sleep(self._min_interval - elapsed)
        self._last = time.monotonic()

    def _fetch_with_retry(self, path: str, url: str) -> str:
        attempt = 0
        while True:
            attempt += 1
            self._wait()
            try:
                response = self._http.get(url, headers={"User-Agent": self._user_agent})
            except _CONFIG_ERROR_TYPES as exc:
                raise _config_error(exc) from exc
            except httpx.HTTPError as exc:
                if attempt <= self._max_retries:
                    time.sleep(_backoff(attempt))
                    continue
                raise _transport_error(exc) from exc

            status = response.status_code
            if 200 <= status < 300:
                return response.text

            retryable = status in RETRYABLE_MB
            if not retryable or attempt > self._max_retries:
                body = _redact_text(response.text)
                if status == 404:
                    raise _mb_not_found(path)
                if retryable:
                    raise MimportError(
                        f"musicbrainz is still rate limited after {attempt} attempts"
                    )
                raise MimportError(f"musicbrainz request failed: HTTP {status}\n{body}")

            sleep_for = _retry_after(response)
            time.sleep(sleep_for if sleep_for is not None else _backoff(attempt))


class LidarrClient:
    def __init__(self, cfg: Any) -> None:
        try:
            self._http = httpx.Client(
                timeout=LIDARR_TIMEOUT_SECS, follow_redirects=True, max_redirects=10
            )
            self._base_url = str(cfg.base_url).rstrip("/")
            self._cache = DiskCache(cfg.cache_dir, int(cfg.cache_ttl_secs))
            self._max_retries = int(cfg.max_retries)
            self._last = time.monotonic() - LIDARR_MIN_INTERVAL_SECS
        except _CLIENT_BUILD_ERRORS as exc:
            raise _config_error(exc) from exc

    def __enter__(self) -> LidarrClient:
        return self

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> bool:
        self.close()
        return False

    def close(self) -> None:
        self._http.close()

    def get(self, path: str, params: list[tuple[str, str]] | None = None) -> Any:
        url = f"{self._base_url}{path}"
        if params:
            url += "?" + "&".join(f"{key}={_urlencode(value)}" for key, value in params)

        cached = self._cache.get(url)
        if cached is not None:
            return _parse_json(cached)

        body = self._fetch_with_retry(path, url)
        value = _parse_json(body)
        self._cache.put(url, body)
        return value

    def _wait(self) -> None:
        elapsed = time.monotonic() - self._last
        if elapsed < LIDARR_MIN_INTERVAL_SECS:
            time.sleep(LIDARR_MIN_INTERVAL_SECS - elapsed)
        self._last = time.monotonic()

    def _fetch_with_retry(self, path: str, url: str) -> str:
        attempt = 0
        while True:
            attempt += 1
            self._wait()
            try:
                response = self._http.get(url, headers={"User-Agent": LIDARR_USER_AGENT})
            except _CONFIG_ERROR_TYPES as exc:
                raise _config_error(exc) from exc
            except httpx.HTTPError as exc:
                if attempt <= self._max_retries:
                    time.sleep(_backoff(attempt))
                    continue
                raise _transport_error(exc) from exc

            status = response.status_code
            if 200 <= status < 300:
                return response.text

            retryable = status in RETRYABLE_LIDARR
            if not retryable or attempt > self._max_retries:
                body = _redact_text(response.text)
                if status == 404:
                    raise _lidarr_not_found(path)
                if retryable:
                    raise MimportError(
                        f"lidarr proxy is still failing after {attempt} attempts"
                    )
                raise MimportError(f"lidarr proxy request failed: HTTP {status}\n{body}")

            sleep_for = _retry_after(response)
            time.sleep(sleep_for if sleep_for is not None else _backoff(attempt))


class SlskdClient:
    def __init__(self, cfg: Any) -> None:
        secrets = (str(cfg.username), str(cfg.password))
        try:
            self._http = httpx.Client(
                timeout=float(cfg.request_timeout_secs),
                follow_redirects=True,
                max_redirects=10,
            )
            self._base_url = str(cfg.base_url).rstrip("/")
            self._username = secrets[0]
            self._password = secrets[1]
            self._token: str | None = None
        except _CLIENT_BUILD_ERRORS as exc:
            raise _config_error(exc, secrets) from exc

    def __enter__(self) -> SlskdClient:
        return self

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> bool:
        self.close()
        return False

    def close(self) -> None:
        self._http.close()

    def _secrets(self) -> tuple[str, ...]:
        return tuple(
            item for item in (self._username, self._password, self._token) if item
        )

    def _login(self) -> str:
        url = f"{self._base_url}/api/v0/session"
        secrets = self._secrets()
        try:
            response = self._http.post(
                url,
                json={"username": self._username, "password": self._password},
            )
        except _CONFIG_ERROR_TYPES as exc:
            raise _config_error(exc, secrets) from exc
        except httpx.HTTPError as exc:
            raise _transport_error(exc, secrets) from exc

        status = response.status_code
        if not (200 <= status < 300):
            raise MimportError(f"slskd login failed: HTTP {status}")

        session = _parse_json(response.text)
        if not isinstance(session, dict):
            raise MimportError("json error: expected object for session")
        token = session.get("token")
        token_type = session.get("tokenType")
        if not isinstance(token, str):
            raise MimportError("json error: missing field `token`")
        if not isinstance(token_type, str):
            raise MimportError("json error: missing field `tokenType`")
        self._token = token
        return token

    def _call(
        self,
        method: str,
        path: str,
        body: Any = _MISSING,
        timeout: float | None = None,
    ) -> Any:
        url = f"{self._base_url}{path}"
        retried_login = False

        while True:
            token = self._token if self._token is not None else self._login()
            secrets = self._secrets()
            headers = {"Authorization": f"Bearer {token}"}
            kwargs: dict[str, Any] = {"headers": headers}
            if timeout is not None:
                kwargs["timeout"] = timeout
            if body is not _MISSING:
                kwargs["json"] = body

            try:
                response = self._http.request(method, url, **kwargs)
            except _CONFIG_ERROR_TYPES as exc:
                raise _config_error(exc, secrets) from exc
            except httpx.HTTPError as exc:
                raise _transport_error(exc, secrets) from exc

            status = response.status_code
            if 200 <= status < 300:
                text = response.text
                if not text.strip():
                    return None
                return _parse_json(text)

            if status == 401 and not retried_login:
                retried_login = True
                self._login()
                continue

            body_text = _redact_body(response.text, secrets)
            if status == 404:
                safe_path = _redact_text(path, secrets)
                raise _SlskdNotFound(
                    f"slskd has no user or peer resource at {safe_path} (peer likely offline)",
                    service="slskd",
                    status=404,
                    resource=safe_path,
                )
            raise MimportError(f"slskd request failed: HTTP {status}\n{body_text}")

    def get(self, path: str, timeout: float | None = None) -> Any:
        return self._call("GET", path, timeout=timeout)

    def get_with_timeout(self, path: str, timeout: float) -> Any:
        return self._call("GET", path, timeout=timeout)

    def post(self, path: str, body: Any, timeout: float | None = None) -> Any:
        return self._call("POST", path, body=body, timeout=timeout)

    def post_with_timeout(self, path: str, body: Any, timeout: float) -> Any:
        return self._call("POST", path, body=body, timeout=timeout)

    def delete(self, path: str, timeout: float | None = None) -> Any:
        return self._call("DELETE", path, timeout=timeout)

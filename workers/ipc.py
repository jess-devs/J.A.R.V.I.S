"""Protocolo de IPC compartido entre Rust y los workers Python.

Debe importarse ANTES que cualquier libreria pesada (torch, RealtimeSTT, piper).
Redirige el file descriptor 1 (stdout) a nivel de sistema operativo hacia el
descriptor 2 (stderr), porque librerias nativas en C/CUDA (torch, onnxruntime,
PortAudio) pueden escribir directo al fd 1 sin pasar por sys.stdout de Python.
Sin este dup2 esos prints corromperian el stream NDJSON del protocolo.
"""

import io
import json
import os
import sys
import threading

_protocol_fd = os.dup(1)
_protocol_out = os.fdopen(_protocol_fd, "wb", buffering=0)

os.dup2(2, 1)
sys.stdout = sys.stderr

_stdin_buffer: io.BufferedReader = sys.stdin.buffer

_write_lock = threading.Lock()
_read_lock = threading.Lock()


def send(msg: dict) -> None:
    """Envia una linea NDJSON por el stream de protocolo real."""
    line = (json.dumps(msg, ensure_ascii=False) + "\n").encode("utf-8")
    with _write_lock:
        _protocol_out.write(line)


def send_audio(header: dict, pcm_bytes: bytes) -> None:
    """Envia un header NDJSON seguido de los bytes crudos de audio."""
    header = {**header, "bytes": len(pcm_bytes)}
    line = (json.dumps(header, ensure_ascii=False) + "\n").encode("utf-8")
    with _write_lock:
        _protocol_out.write(line)
        _protocol_out.write(pcm_bytes)


def read_line() -> dict | None:
    """Lee y parsea una linea NDJSON desde stdin real. None si el stream se cerro."""
    with _read_lock:
        line = _stdin_buffer.readline()
    if not line:
        return None
    return json.loads(line.decode("utf-8"))

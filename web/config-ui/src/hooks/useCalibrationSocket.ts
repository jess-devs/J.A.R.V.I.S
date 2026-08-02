import { useCallback, useEffect, useRef, useState } from 'react';
import type { AudioDeviceInfo, CalibrationClientMessage, CalibrationServerMessage } from '../api/types';

interface ActiveDevice {
  index: number;
  name: string;
}

interface UseCalibrationSocketResult {
  connected: boolean;
  devices: AudioDeviceInfo[];
  activeDevice: ActiveDevice | null;
  dbfs: number | null;
  error: string | null;
  died: boolean;
  start: (deviceIndex: number | null) => void;
  stop: () => void;
}

function sendMessage(socket: WebSocket | null, msg: CalibrationClientMessage) {
  if (socket && socket.readyState === WebSocket.OPEN) {
    socket.send(JSON.stringify(msg));
  }
}

/**
 * Abre el WebSocket de calibración de micrófono al montar y lo cierra al
 * desmontar. Cerrarlo es lo que dispara `CalibrationWorker::shutdown` del
 * lado Rust — si un componente que usa este hook se desmonta sin que el
 * cleanup corra (no debería pasar con `useEffect`, pero es la razón de que
 * esto sea crítico), el proceso Python de calibración quedaría con el
 * micrófono abierto hasta el timeout de inactividad del servidor.
 */
export function useCalibrationSocket(): UseCalibrationSocketResult {
  const socketRef = useRef<WebSocket | null>(null);
  const [connected, setConnected] = useState(false);
  const [devices, setDevices] = useState<AudioDeviceInfo[]>([]);
  const [activeDevice, setActiveDevice] = useState<ActiveDevice | null>(null);
  const [dbfs, setDbfs] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [died, setDied] = useState(false);

  useEffect(() => {
    const protocol = location.protocol === 'https:' ? 'wss:' : 'ws:';
    const socket = new WebSocket(`${protocol}//${location.host}/api/onboarding/calibration/ws`);
    socketRef.current = socket;

    socket.onopen = () => {
      setConnected(true);
      setError(null);
      sendMessage(socket, { type: 'list_devices' });
    };

    socket.onclose = () => setConnected(false);

    socket.onerror = () => {
      setError('No se pudo conectar con Jarvis para calibrar el micrófono.');
    };

    socket.onmessage = (event: MessageEvent<string>) => {
      let msg: CalibrationServerMessage;
      try {
        msg = JSON.parse(event.data) as CalibrationServerMessage;
      } catch {
        return;
      }
      switch (msg.type) {
        case 'devices':
          setDevices(msg.devices);
          break;
        case 'started':
          setActiveDevice({ index: msg.device_index, name: msg.device_name });
          setError(null);
          setDied(false);
          break;
        case 'level':
          setDbfs(msg.dbfs);
          break;
        case 'error':
          setError(msg.message);
          setDbfs(null);
          break;
        case 'died':
          setDied(true);
          setDbfs(null);
          break;
      }
    };

    return () => {
      socket.close();
      socketRef.current = null;
    };
  }, []);

  const start = useCallback((deviceIndex: number | null) => {
    setDied(false);
    sendMessage(socketRef.current, { type: 'start_calibration', device_index: deviceIndex });
  }, []);

  const stop = useCallback(() => {
    sendMessage(socketRef.current, { type: 'stop_calibration' });
    setDbfs(null);
    setActiveDevice(null);
  }, []);

  return { connected, devices, activeDevice, dbfs, error, died, start, stop };
}

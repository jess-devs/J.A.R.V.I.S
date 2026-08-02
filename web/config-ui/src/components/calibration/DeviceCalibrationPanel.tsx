import { AlertTriangle, Mic, MicOff, Pause, Play } from 'lucide-react';
import { Select } from '../form/Controls';
import { useCalibrationSocket } from '../../hooks/useCalibrationSocket';
import { VuMeter } from './VuMeter';
import styles from './DeviceCalibrationPanel.module.css';

const DEFAULT_DEVICE_VALUE = '__default__';

interface DeviceCalibrationPanelProps {
  /** `null` = dispositivo por defecto del sistema (mismo significado que
   * `stt.input_device_index: null` en config.yaml). */
  selectedDeviceIndex: number | null;
  onSelect: (deviceIndex: number | null) => void;
}

/**
 * Componente compartido entre el wizard de primer arranque
 * (`OnboardingWizard`) y la sección "Micrófono" del panel de configuración
 * (`OnboardingSection`): dropdown de dispositivos + VU meter en vivo. No
 * persiste nada él mismo — quien lo embebe decide cuándo y dónde guardar
 * `selectedDeviceIndex` (ver `OnboardingSection`, que lo guarda en
 * `stt.input_device_index`).
 *
 * El micrófono NO se abre solo: hace falta apretar "Escuchar" — entrar a
 * esta pantalla (o a la sección del sidebar) no debe activar la captura de
 * audio por su cuenta, ni siquiera para elegir el dispositivo del
 * desplegable (eso solo necesita la lista de nombres, que sí se pide sola
 * al conectar, sin abrir ningún stream).
 */
export function DeviceCalibrationPanel({ selectedDeviceIndex, onSelect }: DeviceCalibrationPanelProps) {
  const { connected, devices, activeDevice, dbfs, error, died, start, stop } = useCalibrationSocket();
  const listening = activeDevice !== null;

  const options = [
    { value: DEFAULT_DEVICE_VALUE, label: 'Dispositivo por defecto del sistema' },
    ...devices.map((d) => ({
      value: String(d.index),
      label: d.is_default ? `${d.name} (por defecto)` : d.name,
    })),
  ];

  const selectValue = selectedDeviceIndex === null ? DEFAULT_DEVICE_VALUE : String(selectedDeviceIndex);

  const handleChange = (value: string) => {
    const index = value === DEFAULT_DEVICE_VALUE ? null : Number(value);
    onSelect(index);
    // Si ya estabas escuchando, seguís escuchando pero contra el nuevo
    // dispositivo. Si no, solo cambia la selección — elegir de la lista no
    // debe activar el micrófono por su cuenta.
    if (listening) {
      start(index);
    }
  };

  const toggleListening = () => {
    if (listening) {
      stop();
    } else {
      start(selectedDeviceIndex);
    }
  };

  const showError = died || error !== null;

  return (
    <div className={styles.panel}>
      <Select value={selectValue} onChange={handleChange} options={options} />

      <div className={styles.meterRow}>
        {showError ? (
          <div className={styles.errorRow}>
            <AlertTriangle size={16} strokeWidth={2} />
            <span>{error ?? 'El proceso de calibración se detuvo. Probá elegir el dispositivo de nuevo.'}</span>
          </div>
        ) : (
          <>
            {listening ? (
              <Mic size={18} strokeWidth={2} className={styles.micIcon} />
            ) : (
              <MicOff size={18} strokeWidth={2} className={styles.micIconOff} />
            )}
            <VuMeter dbfs={listening ? dbfs : null} />
            <button
              type="button"
              className={styles.listenButton}
              onClick={toggleListening}
              disabled={!connected}
            >
              {listening ? <Pause size={14} strokeWidth={2} /> : <Play size={14} strokeWidth={2} />}
              {listening ? 'Detener' : 'Escuchar'}
            </button>
          </>
        )}
      </div>

      <p className={styles.hint}>
        {!connected
          ? 'Conectando con Jarvis…'
          : listening
            ? `Escuchando "${activeDevice.name}". Hablá normal para ver el nivel.`
            : 'El micrófono no está activo — apretá "Escuchar" para probarlo.'}
      </p>
    </div>
  );
}

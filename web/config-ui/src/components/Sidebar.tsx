import { GROUPS, SECTIONS } from '../config/sections';
import styles from './Sidebar.module.css';

interface SidebarProps {
  activeId: string;
  onSelect: (id: string) => void;
}

export function Sidebar({ activeId, onSelect }: SidebarProps) {
  return (
    <aside className={styles.sidebar}>
      <div className={styles.brand}>
        <span className={styles.brandMark} aria-hidden="true">
          <img src="/robot.webp" alt="" />
        </span>
        <span className={styles.brandName}>Jarvis</span>
      </div>

      <nav className={styles.nav} aria-label="Secciones de configuración">
        {GROUPS.map((group) => (
          <div key={group.id} className={styles.group}>
            <div className={styles.groupHeader}>{group.label}</div>
            {SECTIONS.filter((section) => section.group === group.id).map((section) => {
              const Icon = section.icon;
              const isActive = section.id === activeId;
              return (
                <button
                  key={section.id}
                  type="button"
                  className={[
                    styles.item,
                    isActive ? styles.itemActive : '',
                    !section.enabled ? styles.itemDisabled : '',
                  ].join(' ')}
                  disabled={!section.enabled}
                  aria-current={isActive ? 'page' : undefined}
                  aria-label={section.label}
                  title={section.label}
                  onClick={() => onSelect(section.id)}
                >
                  <Icon size={16} strokeWidth={1.75} className={styles.itemIcon} />
                  <span className={styles.itemLabel}>{section.label}</span>
                  {!section.enabled && <span className={styles.badge}>Pronto</span>}
                </button>
              );
            })}
          </div>
        ))}
      </nav>

      <div className={styles.footer}>
        <span className={styles.footerText}>config.yaml local</span>
      </div>
    </aside>
  );
}

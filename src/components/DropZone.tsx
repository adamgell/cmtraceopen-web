import { useCallback, useRef, useState } from "react";
import { tokens } from "@fluentui/react-components";

export interface DropZoneProps {
  onFile: (file: File) => void;
  disabled?: boolean;
}

/**
 * Drag-and-drop target with a fallback "Choose file" button.
 * Single file at a time; doesn't filter by extension — CMTrace logs come
 * in many flavors (.log, .txt, .cmtlog, no extension, etc.).
 */
export function DropZone({ onFile, disabled = false }: DropZoneProps) {
  const [isDragOver, setIsDragOver] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);

  const handleDragOver = useCallback((e: React.DragEvent<HTMLDivElement>) => {
    e.preventDefault();
    e.stopPropagation();
    if (disabled) return;
    e.dataTransfer.dropEffect = "copy";
    setIsDragOver(true);
  }, [disabled]);

  const handleDragLeave = useCallback((e: React.DragEvent<HTMLDivElement>) => {
    e.preventDefault();
    e.stopPropagation();
    setIsDragOver(false);
  }, []);

  const handleDrop = useCallback((e: React.DragEvent<HTMLDivElement>) => {
    e.preventDefault();
    e.stopPropagation();
    setIsDragOver(false);
    if (disabled) return;
    const file = e.dataTransfer.files?.[0];
    if (file) onFile(file);
  }, [disabled, onFile]);

  const handlePickClick = useCallback(() => {
    inputRef.current?.click();
  }, []);

  const handleInputChange = useCallback((e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    if (file) onFile(file);
    // Reset so picking the same file twice in a row re-fires the event.
    e.target.value = "";
  }, [onFile]);

  return (
    <div
      onDragOver={handleDragOver}
      onDragLeave={handleDragLeave}
      onDrop={handleDrop}
      style={{
        border: `2px dashed ${
          isDragOver ? tokens.colorBrandStroke1 : tokens.colorNeutralStroke2
        }`,
        borderRadius: 8,
        padding: 48,
        textAlign: "center",
        background: isDragOver
          ? tokens.colorBrandBackground2
          : tokens.colorNeutralBackground2,
        color: tokens.colorNeutralForeground1,
        transition: "background 120ms, border-color 120ms",
        cursor: disabled ? "not-allowed" : "default",
        opacity: disabled ? 0.6 : 1,
      }}
    >
      <div style={{ fontSize: 18, marginBottom: 8 }}>
        Drop a log file here
      </div>
      <div
        style={{
          color: tokens.colorNeutralForeground2,
          fontSize: 14,
          marginBottom: 20,
        }}
      >
        .log, .txt, .cmtlog, or anything else CMTrace-shaped
      </div>
      <button
        type="button"
        onClick={handlePickClick}
        disabled={disabled}
        style={{
          padding: "8px 20px",
          fontSize: 14,
          border: `1px solid ${tokens.colorBrandBackground}`,
          background: tokens.colorBrandBackground,
          color: tokens.colorNeutralForegroundOnBrand,
          borderRadius: tokens.borderRadiusMedium,
          cursor: disabled ? "not-allowed" : "pointer",
        }}
      >
        Choose file…
      </button>
      <input
        ref={inputRef}
        type="file"
        onChange={handleInputChange}
        style={{ display: "none" }}
      />
    </div>
  );
}

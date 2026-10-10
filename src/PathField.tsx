import { useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";

/** A draft path is applied only by Enter, Apply, or a completed folder pick. */
export function PathField({
  label,
  value,
  placeholder,
  disabled = false,
  pickerLabel,
  onApply,
}: {
  label: string;
  value: string;
  placeholder?: string;
  disabled?: boolean;
  pickerLabel: string;
  onApply: (path: string) => void;
}) {
  const [draft, setDraft] = useState<string | null>(null);
  const next = draft ?? value;

  const apply = (path: string) => {
    setDraft(null);
    onApply(path);
  };
  const commitDraft = () => {
    if (!disabled && next !== value) apply(next);
  };
  const pick = async () => {
    const picked = await open({ directory: true, multiple: false });
    if (typeof picked === "string") apply(picked);
  };

  return (
    <>
      <input
        className="path"
        aria-label={label}
        value={next}
        placeholder={placeholder}
        disabled={disabled}
        onChange={(event) => setDraft(event.currentTarget.value)}
        onKeyDown={(event) => {
          if (event.key === "Enter" && !event.nativeEvent.isComposing) {
            event.preventDefault();
            commitDraft();
          }
        }}
      />
      <button
        type="button"
        aria-label={`Apply ${label}`}
        disabled={disabled || next === value}
        onClick={commitDraft}
      >
        Apply
      </button>
      <button type="button" onClick={pick} disabled={disabled}>
        {pickerLabel}
      </button>
    </>
  );
}

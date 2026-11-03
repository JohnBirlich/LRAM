import { useEffect, useState, type ReactNode } from "react";
import { toast } from "./Toast";
import { t } from "../i18n";

export function Switch({ checked, onChange, label }: { checked: boolean; onChange: (v: boolean) => void; label: string }) {
  return (
    <button type="button" role="switch" className="switch" aria-checked={checked} aria-label={label} onClick={() => onChange(!checked)} />
  );
}

export function Segmented<T extends string | number>({ label, value, options, onChange, mono }: {
  label: string;
  value: T;
  options: { value: T; label: ReactNode; disabled?: boolean; title?: string }[];

  onChange: (v: T) => void;
  mono?: boolean;
}) {
  return (
    <div role="group" aria-label={label} className="seg-group">
      {options.map((o) => (
        <button
          key={o.value}
          type="button"
          className={mono ? "seg mono" : "seg"}
          disabled={o.disabled}
          title={o.title}
          aria-pressed={o.value === value}
          onClick={() => o.value !== value && onChange(o.value)}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

export function TextField({ label, value, placeholder, className = "", onCommit }: {
  label: string;
  value: string;
  placeholder?: string;
  className?: string;
  onCommit: (text: string) => Promise<unknown>;
}) {
  const [text, setText] = useState(value);
  useEffect(() => setText(value), [value]);
  const commit = () => {
    const [next, before] = [text.trim(), value];
    if (next !== before) onCommit(next).catch(() => setText(before));
  };
  return (
    <input
      className={`field mono ${className}`}
      aria-label={label}
      value={text}
      placeholder={placeholder}
      spellCheck={false}
      autoComplete="off"
      onChange={(e) => setText(e.target.value)}
      onBlur={commit}
      onKeyDown={(e) => {
        if (e.key === "Enter") e.currentTarget.blur();
        if (e.key === "Escape" && text !== value) {
          e.stopPropagation();
          setText(value);
        }
      }}
    />
  );
}

// -- Main func
export function NumberField({ label, unit, short, min, max, step = 1, value, onCommit }: {

  label?: string;
  unit: string;
  short: string;
  min: number;
  max: number;
  step?: number;
  value: number;
  onCommit: (n: number) => void;
}) {
  const [text, setText] = useState(String(value));
  useEffect(() => setText(String(value)), [value]);
  const commit = () => {
    const n = Number(text);
    if (n === value) return;
    if (Number.isInteger(n / step) && n >= min && n <= max) onCommit(n);
    else {
      toast(step === 1 ? t("ui.numberWhole", { unit, min, max }) : t("ui.numberStep", { unit, min, max, step }), "error");
      setText(String(value));
    }
  };
  return (
    <label className="flex items-center gap-2">
      {label ? <span>{label}</span> : <span className="sr-only">{unit}</span>}
      <input
        className="field mono w-20"
        type="number"
        min={min}
        max={max}
        step={step}
        value={text}
        onChange={(e) => setText(e.target.value)}
        onBlur={commit}
        onWheel={(e) => e.currentTarget.blur()}
        onKeyDown={(e) => e.key === "Enter" && e.currentTarget.blur()}
      />
      <span className="text-dim">{short}</span>
    </label>
  );
}

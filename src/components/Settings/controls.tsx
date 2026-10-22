// Controles da tela de configurações (controlados, sem estado próprio).
import { type ComponentChildren } from "preact";
import { useState } from "preact/hooks";
import Icon from "../Overlay/Icon";
import { useT } from "../../i18n";

export function Section({ title, description, children }: { title: string; description?: ComponentChildren; children: ComponentChildren }) {
    return (
        <section className="flex flex-col gap-3">
            <div>
                <h2 className="text-lg font-semibold">{title}</h2>
                {description && <p className="text-sm text-(--text-muted)">{description}</p>}
            </div>
            <div className="flex flex-col rounded-xl bg-dark/60 [border:var(--border)] divide-y divide-(--background-color-normal)">{children}</div>
        </section>
    );
}

export function Row({ label, hint, children }: { label: ComponentChildren; hint?: ComponentChildren; children?: ComponentChildren }) {
    return (
        <div className="flex items-center gap-4 px-4 py-3 min-h-14">
            <div className="flex-1 min-w-0">
                <div>{label}</div>
                {hint && <div className="text-sm text-(--text-muted)">{hint}</div>}
            </div>
            {children && <div className="flex items-center gap-3 shrink-0">{children}</div>}
        </div>
    );
}

export function Toggle({ checked, onChange, disabled }: { checked: boolean; onChange: (value: boolean) => void; disabled?: boolean }) {
    return (
        <button
            role="switch"
            aria-checked={checked}
            disabled={disabled}
            onClick={() => onChange(!checked)}
            className={`relative w-11 h-6 rounded-full transition cursor-pointer disabled:opacity-40 disabled:cursor-not-allowed ${checked ? "bg-accent" : "bg-normal"}`}>
            <span className={`absolute top-0.5 left-0.5 w-5 h-5 rounded-full bg-white transition-transform ${checked ? "translate-x-5" : ""}`} />
        </button>
    );
}

export type Option<T extends string> = { value: T; label: string; disabled?: boolean };

export function Select<T extends string>({ value, options, onChange, width = "w-64" }: { value: T; options: Option<T>[]; onChange: (value: T) => void; width?: string }) {
    return (
        <select
            value={value}
            onChange={(e) => onChange(e.currentTarget.value as T)}
            className={`${width} h-9 px-2 rounded-lg bg-dark [border:var(--border)] cursor-pointer outline-none focus:border-accent`}>
            {options.map((option) => (
                <option key={option.value} value={option.value} disabled={option.disabled} className="bg-dark">
                    {option.label}
                </option>
            ))}
        </select>
    );
}

export function Slider({
    value,
    min,
    max,
    step = 1,
    onChange,
    format = String,
}: {
    value: number;
    min: number;
    max: number;
    step?: number;
    onChange: (value: number) => void;
    format?: (value: number) => string;
}) {
    return (
        <div className="flex items-center gap-3">
            <input type="range" min={min} max={max} step={step} value={value} onInput={(e) => onChange(Number(e.currentTarget.value))} className="w-40 accent-accent cursor-pointer" />
            <span className="w-16 text-right tabular-nums text-sm">{format(value)}</span>
        </div>
    );
}

/** Número aplicado ao sair do campo ou apertar Enter (não a cada tecla). */
export function NumberInput({ value, min, max, step = 1, onChange, suffix }: { value: number; min: number; max: number; step?: number; onChange: (value: number) => void; suffix?: string }) {
    const [draft, setDraft] = useState<string | null>(null);
    const commit = () => {
        if (draft === null) return;
        const parsed = Number(draft);
        if (Number.isFinite(parsed)) onChange(Math.min(max, Math.max(min, parsed)));
        setDraft(null);
    };
    return (
        <div className="flex items-center gap-2">
            <input
                type="number"
                min={min}
                max={max}
                step={step}
                value={draft ?? String(value)}
                onInput={(e) => setDraft(e.currentTarget.value)}
                onBlur={commit}
                onKeyDown={(e) => e.key === "Enter" && commit()}
                className="w-24 h-9 px-2 rounded-lg bg-dark [border:var(--border)] outline-none focus:border-accent tabular-nums"
            />
            {suffix && <span className="text-sm text-(--text-muted)">{suffix}</span>}
        </div>
    );
}

export function Segmented<T extends string>({ value, options, onChange }: { value: T; options: Option<T>[]; onChange: (value: T) => void }) {
    return (
        <div className="flex rounded-lg bg-dark [border:var(--border)] p-0.5">
            {options.map((option) => (
                <button
                    key={option.value}
                    disabled={option.disabled}
                    onClick={() => onChange(option.value)}
                    className={`px-3 h-8 rounded-md text-sm transition cursor-pointer disabled:opacity-40 disabled:cursor-not-allowed ${
                        value === option.value ? "bg-accent text-white" : "hover:bg-normal"
                    }`}>
                    {option.label}
                </button>
            ))}
        </div>
    );
}

export function Button({
    children,
    onClick,
    variant = "plain",
    disabled,
    title,
}: {
    children: ComponentChildren;
    onClick: () => void;
    variant?: "plain" | "accent" | "danger";
    disabled?: boolean;
    title?: string;
}) {
    const colors = { plain: "bg-normal hover:brightness-125", accent: "bg-accent hover:brightness-110", danger: "bg-bad/80 hover:bg-bad" }[variant];
    return (
        <button title={title} disabled={disabled} onClick={onClick} className={`h-9 px-4 rounded-lg transition cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed ${colors}`}>
            {children}
        </button>
    );
}

export function CopyField({ value, fullWidth = false }: { value: string; fullWidth?: boolean }) {
    const t = useT();
    const [copied, setCopied] = useState(false);
    return (
        <div className={`flex items-center gap-2 min-w-0 ${fullWidth ? "w-full" : ""}`}>
            <code className={`px-2 h-9 flex items-center rounded-lg bg-dark [border:var(--border)] text-sm select-all min-w-0 truncate ${fullWidth ? "flex-1" : "max-w-72"}`}>
                {value}
            </code>
            <button
                title={t("common.copy")}
                onClick={() => navigator.clipboard.writeText(value).then(() => (setCopied(true), setTimeout(() => setCopied(false), 1500)))}
                className="w-9 h-9 shrink-0 flex items-center justify-center rounded-lg bg-normal hover:brightness-125 cursor-pointer">
                <Icon src={copied ? "./assets/Alerts/check.svg" : "./assets/Settings/copy.svg"} color={copied ? "var(--color-accent)" : undefined} />
            </button>
        </div>
    );
}

export function Badge({ children, color = "var(--background-color-normal)" }: { children: ComponentChildren; color?: string }) {
    return (
        <span className="px-2 py-0.5 rounded-md text-xs font-semibold" style={{ background: color }}>
            {children}
        </span>
    );
}

export function Note({ children, tone = "info" }: { children: ComponentChildren; tone?: "info" | "warning" | "error" }) {
    const border = { info: "border-info/50 bg-info/10", warning: "border-[goldenrod]/50 bg-[goldenrod]/10", error: "border-bad/50 bg-bad/10" }[tone];
    return <div className={`px-4 py-3 rounded-xl border text-sm ${border}`}>{children}</div>;
}

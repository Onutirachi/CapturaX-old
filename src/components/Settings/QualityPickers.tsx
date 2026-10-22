// Seletores de resolução e FPS com as bandeirinhas do app (mesmas listas da barra superior),
// mais a opção "Native" (igual à fonte).
import { DropDown, DropOption } from "../../core/components/DropDown";
import Tag from "../../core/components/Tag";
import Resolution from "../../core/utils/Resolution";
import { FRAMERATE_LIST, RESOLUTION_LIST } from "../../core/utils/constants";
import type { SourceInfo } from "../../core/media/presets";
import { fpsColor, formatFps } from "../../core/utils/videoModes";
import { useT } from "../../i18n";

const NATIVE = "native";

export type Size = { width: number; height: number };

const key = (size: Size) => `${size.width}x${size.height}`;

/** `value = null` = nativo. Só oferece resoluções até a da fonte (nunca aumenta a imagem). */
export function ResolutionPicker({ value, source, onChange, fullWidth }: { value: Size | null; source: SourceInfo; onChange: (value: Size | null) => void; fullWidth?: boolean }) {
    const t = useT();
    const nativeTag = Resolution.getTag(source);
    const options = RESOLUTION_LIST.filter((r) => r.width <= source.width && r.height <= source.height);
    const selected = value ? key(value) : NATIVE;
    // Valor salvo fora da lista (ex.: a fonte mudou): continua aparecendo.
    const custom = value && !options.some((r) => key(r) === selected) ? value : null;
    return (
        <DropDown
            title={t("toolbar.resolution")}
            fullWidth={fullWidth}
            value={selected}
            onChoose={(v) => {
                if (v === NATIVE) return onChange(null);
                const [width, height] = v.split("x").map(Number);
                onChange({ width, height });
            }}>
            <DropOption value={NATIVE}>
                {t("quality.native", { size: `${source.width} x ${source.height}` })}
                <Tag text={nativeTag.text} color={nativeTag.color} />
            </DropOption>
            {custom && (
                <DropOption value={key(custom)}>
                    {`${custom.width} x ${custom.height}`}
                    <Tag text={Resolution.getTag(custom).text} color={Resolution.getTag(custom).color} />
                </DropOption>
            )}
            {options.map((r) => (
                <DropOption key={key(r)} value={key(r)}>
                    {`${r.width} x ${r.height}`}
                    <Tag text={r.tag} color={r.color} />
                </DropOption>
            ))}
        </DropDown>
    );
}

/** `value = null` = o FPS da fonte, que aparece marcado na própria lista (sem opção "Nativo"). */
export function FpsPicker({ value, source, onChange, fullWidth }: { value: number | null; source: SourceInfo; onChange: (value: number | null) => void; fullWidth?: boolean }) {
    const t = useT();
    const sourceFps = Math.round(source.fps);
    const options = [...new Set([sourceFps, ...FRAMERATE_LIST.filter((f) => f <= sourceFps), ...(value ? [value] : [])])].sort((a, b) => b - a);
    return (
        <DropDown title={t("toolbar.framerate")} fullWidth={fullWidth} value={String(value ?? sourceFps)} onChoose={(v) => onChange(Number(v) === sourceFps ? null : Number(v))}>
            {options.map((f) => (
                <DropOption key={f} value={String(f)}>
                    <Tag text={`${formatFps(f)} FPS`} color={fpsColor(f)} />
                </DropOption>
            ))}
        </DropDown>
    );
}

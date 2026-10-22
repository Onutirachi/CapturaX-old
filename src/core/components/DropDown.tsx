import { ComponentChildren } from "preact";
import { useState, useEffect, useLayoutEffect, useRef } from "preact/hooks";
import { Children } from "preact/compat";
import ScrollText from "./ScrollText";

type DropDownProps = {
    children: ComponentChildren;
    title: string;
    onChoose?: (value: string) => void; // eslint-disable-line
    /** Chave do localStorage (padrão: o título). */
    storageKey?: string;
    /** Modo controlado: o valor vem de fora (ex.: config do Rust) e nada é salvo aqui. */
    value?: string | null;
    /** Chamado ao abrir o menu (ex.: atualizar a lista de janelas). */
    onOpen?: () => void;
    /** Travado (mostra o valor, mas não abre). */
    disabled?: boolean;
    /** Ocupa a largura toda do pai (ex.: janelas); sem isso, cresce até um limite e o texto rola. */
    fullWidth?: boolean;
};

/** Evento global para fechar dropdowns abertos (ex.: quando a barra que os contém se esconde). */
export const CLOSE_DROPDOWNS_EVENT = "capturax:close-dropdowns";

type DropOptionProps = {
    children: ComponentChildren;
    className?: string;
    value: string;
};

const isText = (child: unknown) => typeof child === "string" || typeof child === "number";

/**
 * Uma célula de texto (rola se não couber) seguida das bandeirinhas. O próprio elemento não gera
 * caixa (`contents`): no menu aberto, texto e bandeirinhas viram colunas alinhadas entre as opções.
 */
export function DropOption({ children, className, value }: DropOptionProps) {
    const parts = Children.toArray(children);
    const text = parts.filter(isText).join("");
    return (
        <div data-value={value} className={`contents ${className ? className : ""}`}>
            {text ? <ScrollText>{text}</ScrollText> : <span />}
            {parts.filter((child) => !isText(child))}
        </div>
    );
}

/** Área visível onde o menu precisa caber: o primeiro ancestral que corta/rola (ex.: a lista das
 * configurações) ou a janela. */
function visibleArea(element: HTMLElement) {
    for (let parent = element.parentElement; parent; parent = parent.parentElement) {
        const style = getComputedStyle(parent);
        if (/auto|scroll|hidden|clip/.test(style.overflowX + style.overflowY)) {
            const r = parent.getBoundingClientRect();
            return { left: Math.max(r.left, 0), top: Math.max(r.top, 0), right: Math.min(r.right, window.innerWidth), bottom: Math.min(r.bottom, window.innerHeight) };
        }
    }
    return { left: 0, top: 0, right: window.innerWidth, bottom: window.innerHeight };
}

const MARGIN = 8;
const MENU_MAX_WIDTH = 448;
const MENU_MAX_HEIGHT = 400;

type Placement = { right: boolean; up: boolean; maxWidth: number; maxHeight: number };

/** Título de grupo dentro do menu (ex.: "Screens"); não é uma opção. */
export function DropHeader({ children }: { children: ComponentChildren }) {
    return <>{children}</>;
}

const isHeader = (child: any) => child?.type === DropHeader;
/** Só as opções (sem os títulos de grupo). */
const optionsOf = (children: ComponentChildren) => (Children.toArray(children) as any[]).filter((child) => !isHeader(child));

/** Quantas bandeirinhas (colunas além do texto) a opção com mais delas tem. */
function tagColumns(children: ComponentChildren) {
    return Math.max(0, ...optionsOf(children).map((child) => Children.toArray(child?.props?.children).filter((part) => !isText(part)).length));
}

export function DropDown({ children, title, onChoose = (_) => {}, storageKey = title, value: controlledValue, fullWidth = false, onOpen, disabled = false }: DropDownProps) {
    const controlled = controlledValue !== undefined;
    const dropDownRef = useRef<HTMLDivElement | null>(null);
    const [open, setOpen] = useState<boolean>(false);
    const [value, setValue] = useState<string | null>(null);
    const [display, setDisplay] = useState<any | null>(null);
    /** Lado e direção do menu, para caber na área visível sem gerar rolagem. */
    const [placement, setPlacement] = useState<Placement>({ right: false, up: false, maxWidth: MENU_MAX_WIDTH, maxHeight: MENU_MAX_HEIGHT });
    const menuRef = useRef<HTMLDivElement | null>(null);

    const toggleOpen = () => {
        if (disabled) return;
        if (!open) onOpen?.();
        setOpen(!open);
    };

    const selectOption = (child: any) => {
        setOpen(false);
        if (controlled) {
            if (child.props.value !== controlledValue) onChoose(child.props.value);
            return;
        }
        setDisplay(child);
        setValue(child.props.value);
        localStorage.setItem(storageKey, child.props.value);
    };

    // Só re-seleciona quando as opções mudam de verdade (ex.: dispositivo plugado/desplugado).
    // `children` é um array novo a cada render do pai, então não pode ser a dependência.
    const optionValues = optionsOf(children).map((child) => child.props?.value).join("\u0000");

    useEffect(() => {
        if (controlled) return;
        const storedValue = localStorage.getItem(storageKey);
        const optionsArray = optionsOf(children);

        if (storedValue) {
            const found = optionsArray.find((child: any) => child.props?.value === storedValue);
            if (found) {
                selectOption(found);
                return;
            }
        }

        if (optionsArray.length === 0) return;
        selectOption(optionsArray[0]);
    }, [optionValues]);

    useEffect(() => {
        if (controlled || !value) return;
        onChoose(value);
    }, [value]);

    const shown = controlled ? optionsOf(children).find((child) => child.props?.value === controlledValue) ?? null : display;

    useLayoutEffect(() => {
        if (!open || !menuRef.current || !dropDownRef.current) return;
        const menu = menuRef.current;
        const box = dropDownRef.current.getBoundingClientRect();
        const area = visibleArea(dropDownRef.current);
        // Largura natural (a do maior texto), sem o limite da última vez.
        const previous = menu.style.maxWidth;
        menu.style.maxWidth = `${MENU_MAX_WIDTH}px`;
        const width = menu.offsetWidth;
        menu.style.maxWidth = previous;
        const height = Math.min(menu.scrollHeight, MENU_MAX_HEIGHT);
        const spaceRight = area.right - box.left - MARGIN;
        const spaceLeft = box.right - area.left - MARGIN;
        const right = width > spaceRight && spaceLeft > spaceRight;
        const spaceBelow = area.bottom - box.bottom - MARGIN * 2;
        const spaceAbove = box.top - area.top - MARGIN * 2;
        const up = height > spaceBelow && spaceAbove > spaceBelow;
        setPlacement({
            right,
            up,
            maxWidth: Math.max(box.width, Math.min(MENU_MAX_WIDTH, right ? spaceLeft : spaceRight)),
            maxHeight: Math.max(120, Math.min(MENU_MAX_HEIGHT, up ? spaceAbove : spaceBelow)),
        });
    }, [open]);

    useEffect(() => {
        if (!open) return;
        // Captura no document roda antes do listener de Esc do fullscreen (window, bubble).
        const handler = (e: KeyboardEvent) => {
            if (e.key !== "Escape") return;
            e.stopPropagation();
            setOpen(false);
        };
        document.addEventListener("keydown", handler, true);
        return () => document.removeEventListener("keydown", handler, true);
    }, [open]);

    useEffect(() => {
        if (disabled) setOpen(false);
    }, [disabled]);

    useEffect(() => {
        const close = () => setOpen(false);
        window.addEventListener(CLOSE_DROPDOWNS_EVENT, close);
        return () => window.removeEventListener(CLOSE_DROPDOWNS_EVENT, close);
    }, []);

    useEffect(() => {
        const handler = (e: MouseEvent) => {
            if (dropDownRef.current && !dropDownRef.current.contains(e.target as any)) {
                setOpen(false);
            }
        };

        document.addEventListener("click", handler);
        return () => {
            document.removeEventListener("click", handler);
        };
    }, [dropDownRef]);

    return (
        <div data-value={controlled ? controlledValue : value} className={`h-11 relative ${fullWidth ? "w-full" : "w-auto min-w-48 max-w-64"} ${disabled ? "opacity-50" : ""}`} ref={dropDownRef}>
            <div className="absolute bg-normal -translate-y-[50%] translate-x-[20%] px-1 rounded-sm text-sm z-1">{title}</div>
            <div
                className={`w-full h-full bg-dark [border:var(--border)] rounded-xl overflow-hidden ${disabled ? "cursor-not-allowed" : "cursor-pointer"} flex items-center gap-3 px-3 relative`}
                onClick={toggleOpen}>
                {/* Todas as opções empilhadas na mesma célula (só a escolhida visível): a largura do
                    dropdown é sempre a da maior opção, sem quebrar linha nem pular ao trocar. */}
                <div className="grid grid-cols-[minmax(0,1fr)] min-w-0 flex-1">
                    {optionsOf(children).map((child: any) => (
                        <div key={child.props.value} className={`[grid-area:1/1] min-w-0 whitespace-nowrap flex items-center gap-3 [&>:first-child]:flex-1 [&>*]:min-w-0 ${child === shown || (shown && child?.props?.value === (shown as any).props?.value) ? "" : "invisible"}`}>
                            {child}
                        </div>
                    ))}
                </div>
                <div
                    className={`h-[50%] aspect-square ml-auto shrink-0 bg-center mask-contain mask-no-repeat mask-[url(/assets/DropDown/chevron.svg)] bg-white transition ${
                        open ? "-rotate-180" : "rotate-0"
                    }`}></div>
            </div>
            <div
                ref={menuRef}
                style={{ maxWidth: placement.maxWidth }}
                className={`
                    z-2
                    absolute ${placement.up ? "bottom-full mb-2" : "top-full mt-2"} ${placement.right ? "right-0" : "left-0"}
                    w-max min-w-full h-auto
                    p-1.5
                    rounded-xl bg-dark
                    box-border
                    transition-all transition-discrete
                    ${placement.up ? "starting:translate-y-2.5" : "starting:-translate-y-2.5"} starting:opacity-0
                    ${open ? "block opacity-100 translate-y-0" : `hidden opacity-0 pointer-events-none ${placement.up ? "translate-y-2.5" : "-translate-y-2.5"}`}
                `}>
                {/* Fechado é `display: none` (não só transparente): senão a área ocupada gera rolagem. */}
                <div
                    className="w-full h-max overflow-y-auto overflow-x-hidden grid auto-rows-auto"
                    style={{ gridTemplateColumns: `minmax(0, 1fr) repeat(${tagColumns(children)}, max-content)`, maxHeight: placement.maxHeight - 12 }}>
                    {Children.map(children, (child: any) => {
                        if (isHeader(child))
                            return (
                                <div className="col-span-full px-1.5 pt-2 pb-1 text-xs font-semibold uppercase tracking-wide text-(--text-muted) first:pt-0.5">
                                    {child.props.children}
                                </div>
                            );
                        return (
                            <div
                                key={child.props.value}
                                onClick={() => {
                                    selectOption(child);
                                }}
                                className="scroll-on-hover col-span-full grid grid-cols-subgrid gap-x-3 items-center bg-transparent hover:bg-normal cursor-pointer p-1.5 rounded-md box-border whitespace-nowrap min-w-0 [&_.tag]:w-auto">
                                {child}
                            </div>
                        );
                    })}
                </div>
            </div>
        </div>
    );
}

import { ComponentChildren } from "preact";
import preact from "preact";

type ButtonProps = {
    children: ComponentChildren;
    title: string;
    color?: string;
    hoverColor?: string;
    onClick: () => void;
};

type ButtonContentProps = {
    icon: string;
    color?: string;
    hoverColor?: string;
};

export function ButtonContent({ icon, color = "transparent", hoverColor = "transparent" }: ButtonContentProps) {
    const className = `w-full h-[80%] flex items-center justify-center mask-contain mask-center mask-no-repeat mask-(--icon) bg-(--color) group-hover:bg-(--hover-color) transition`;
    const style = {
        // Endereço completo: `url()` relativo em variável CSS é resolvido a partir do arquivo CSS.
        "--icon": `url("${new URL(icon, document.baseURI).href}")`,
        "--color": color,
        "--hover-color": hoverColor,
    } as preact.CSSProperties;
    return <div className={className} style={style}></div>;
}

export function Button({ children, title, color = "transparent", hoverColor = "transparent", onClick }: ButtonProps) {
    const className = `group flex items-center justify-center h-full w-11 rounded-md bg-(--color) hover:bg-(--hover-color) transition `;
    const style = {
        "--color": color,
        "--hover-color": hoverColor,
    } as preact.CSSProperties;

    return (
        <button onClick={onClick} title={title} className={className} style={style}>
            {children}
        </button>
    );
}

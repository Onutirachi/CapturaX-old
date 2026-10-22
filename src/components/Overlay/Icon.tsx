import preact from "preact";

export default function Icon({ src, color }: { src: string; color?: string }) {
    // Endereço completo: um `url()` relativo dentro de variável CSS é resolvido a partir do arquivo
    // CSS que a usa (`/assets/index.css` no executável), não da página.
    const absolute = new URL(src, document.baseURI).href;
    return (
        <div
            className="h-[60%] aspect-square mask-center mask-cover mask-no-repeat bg-(--color) mask-(--src)"
            style={{ "--src": `url("${absolute}")`, "--color": color || "var(--text)" } as preact.CSSProperties}></div>
    );
}

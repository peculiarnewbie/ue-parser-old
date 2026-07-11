import { createSignal, type JSX } from "solid-js";

type DropZoneProps = {
  accept: string;
  label: string;
  hint: string;
  busy?: boolean;
  onFile: (file: File) => void | Promise<void>;
};

export function DropZone(props: DropZoneProps) {
  const [dragging, setDragging] = createSignal(false);
  let inputRef: HTMLInputElement | undefined;

  const takeFile = async (file: File | undefined) => {
    if (!file || props.busy) return;
    await props.onFile(file);
  };

  const onDrop: JSX.EventHandlerUnion<HTMLDivElement, DragEvent> = async (event) => {
    event.preventDefault();
    setDragging(false);
    const file = event.dataTransfer?.files?.[0];
    await takeFile(file);
  };

  return (
    <div
      class="dropzone"
      classList={{ dragging: dragging(), busy: !!props.busy }}
      onDragEnter={(event) => {
        event.preventDefault();
        setDragging(true);
      }}
      onDragOver={(event) => event.preventDefault()}
      onDragLeave={() => setDragging(false)}
      onDrop={onDrop}
      onClick={() => inputRef?.click()}
      role="button"
      tabIndex={0}
      onKeyDown={(event) => {
        if (event.key === "Enter" || event.key === " ") {
          event.preventDefault();
          inputRef?.click();
        }
      }}
    >
      <input
        ref={inputRef}
        type="file"
        accept={props.accept}
        hidden
        disabled={props.busy}
        onChange={async (event) => {
          const file = event.currentTarget.files?.[0];
          event.currentTarget.value = "";
          await takeFile(file);
        }}
      />
      <p class="dropzone-kicker">{props.busy ? "Parsing…" : "Drop file"}</p>
      <h2 class="dropzone-label">{props.label}</h2>
      <p class="dropzone-hint">{props.hint}</p>
    </div>
  );
}

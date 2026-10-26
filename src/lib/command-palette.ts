export interface PaletteCommand {
  id: string;
  label: string;
  detail: string;
  keywords: string[];
  perform: () => void;
}

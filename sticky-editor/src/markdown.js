import DOMPurify from "dompurify";
import MarkdownIt from "markdown-it";
import taskLists from "markdown-it-task-lists";

const md = new MarkdownIt({ html: false, linkify: true, breaks: true }).use(taskLists, {
  enabled: true,
});
const REMOTE = /^([a-z][a-z0-9+.-]+:|[\\/]{2})/i;

function decoded(src) {
  try {
    return decodeURIComponent(src);
  } catch {
    return src;
  }
}

// Checkboxes remember their source line, so ticking one can edit the text.
md.core.ruler.after("github-task-lists", "task-lines", (state) => {
  for (const token of state.tokens) {
    const box = token.children?.[0];
    if (box?.type === "html_inline" && box.content.includes("task-list-item-checkbox")) {
      box.content = box.content.replace("<input ", `<input data-line="${token.map[0]}" `);
    }
  }
});

// Remote images never load: a file someone sent you must not be able to phone
// home, so each one becomes a link the reader can choose to follow. Local ones
// are left for the page to fill in, because the webview can't read files.
md.renderer.rules.image = (tokens, index) => {
  const token = tokens[index];
  const alt = md.utils.escapeHtml(token.content || "image");
  const raw = token.attrGet("src") ?? "";
  if (!REMOTE.test(raw)) {
    return `<img data-src="${md.utils.escapeHtml(decoded(raw))}" alt="${alt}">`;
  }
  return `<a class="image-link" href="${md.utils.escapeHtml(raw)}">[${alt}]</a>`;
};

export function render(text) {
  return DOMPurify.sanitize(md.render(text));
}

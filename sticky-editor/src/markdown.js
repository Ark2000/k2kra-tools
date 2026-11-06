import DOMPurify from "dompurify";
import MarkdownIt from "markdown-it";
import taskLists from "markdown-it-task-lists";

const md = new MarkdownIt({ html: false, linkify: true, breaks: true }).use(taskLists);

// Images never load: a file someone sent you must not be able to phone home,
// so each one becomes a link the reader can choose to follow.
md.renderer.rules.image = (tokens, index) => {
  const token = tokens[index];
  const alt = md.utils.escapeHtml(token.content || "image");
  const src = md.utils.escapeHtml(token.attrGet("src") ?? "");
  return `<a class="image-link" href="${src}">[${alt}]</a>`;
};

export function render(text) {
  return DOMPurify.sanitize(md.render(text));
}

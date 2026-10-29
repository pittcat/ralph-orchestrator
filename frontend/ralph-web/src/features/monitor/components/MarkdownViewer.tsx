/**
 * Sanitized markdown preview (Plan2-U5 S6): react-markdown renders
 * no raw HTML by default; additionally filter dangerous URL schemes
 * (javascript:/data:text/html etc.) on links and images.
 */

import ReactMarkdown from "react-markdown";

function safeUrl(url: string | undefined): string {
  if (!url) return "";
  const trimmed = url.trim().toLowerCase();
  if (
    trimmed.startsWith("javascript:") ||
    trimmed.startsWith("data:") ||
    trimmed.startsWith("vbscript:")
  ) {
    return "#";
  }
  return url;
}

export function MarkdownViewer({ content }: { content: string }) {
  return (
    <div
      className="prose prose-sm max-w-none text-foreground"
      data-testid="markdown-viewer"
    >
      <ReactMarkdown
        components={{
          a: ({ href, children, ...props }) => (
            <a
              href={safeUrl(href)}
              rel="noreferrer noopener"
              className="text-primary underline"
              {...props}
            >
              {children}
            </a>
          ),
          img: ({ src, alt, ...props }) => (
            // eslint-disable-next-line @next/next/no-img-element
            <img src={safeUrl(src)} alt={alt ?? ""} {...props} />
          ),
        }}
      >
        {content}
      </ReactMarkdown>
    </div>
  );
}

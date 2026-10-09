import { useState } from 'react';
import { chatImageLocksBubbleWidth } from '@/components/chatImage.logic';

/**
 * `img` renderer for every chat markdown surface (the answer bubble and the
 * tool-call group share it, so an image behaves identically in both).
 *
 * Why a component rather than a bare `<img>` with CSS: a tall portrait is not
 * constrained by `max-width`. A short height cap with `width: auto` keeps the
 * ratio by shrinking the width, so a screenshot becomes a sliver. When that
 * would happen, the image keeps the bubble width and its own ratio
 * (`.chat-image-tall`) — the whole picture stays visible, nothing is cropped.
 * Clicking opens the full-size original.
 *
 * A new tab rather than an in-page lightbox: enlarging a chat screenshot is the
 * same intent as "show me this file", and the browser's own image viewer gives
 * zoom, pan, and save for free. An in-page `<dialog>` could only re-present the
 * same pixels at viewport size, which does not recover the detail a height cap
 * just removed. `noopener` because the target is attacker-influenced content.
 *
 * The bubble must stay navigable: `target="_blank"` on the image itself would
 * be inert inside the transcript's `translate="no"` region without an anchor,
 * so the click is handled explicitly instead of wrapping it in one.
 */
function bubbleCapWidth(img: HTMLImageElement): number {
  // The transcript is the definite width. The bubble itself shrinks to the
  // image, so measuring the image's parent reports the sliver we are trying
  // to avoid. 85% matches the message column's max-width.
  const transcript = img.closest('.chat-transcript');
  if (transcript instanceof HTMLElement && transcript.clientWidth > 0) {
    return transcript.clientWidth * 0.85;
  }
  return img.parentElement?.clientWidth ?? 0;
}

export default function ChatImage(props: React.ImgHTMLAttributes<HTMLImageElement>) {
  const src = props.src;
  const [lockWidth, setLockWidth] = useState(false);

  // An image with no source (a malformed marker, or a path the loader could not
  // resolve) has nothing to open — render it plainly so the browser's own
  // broken-image state is what the reader sees.
  if (!src) return <img {...props} />;

  const openOriginal = () => {
    // `noopener` + no `window.opener` handoff: the URL is chat content.
    window.open(src, '_blank', 'noopener,noreferrer');
  };

  const onLoad = (e: React.SyntheticEvent<HTMLImageElement>) => {
    props.onLoad?.(e);
    const img = e.currentTarget;
    setLockWidth(chatImageLocksBubbleWidth(bubbleCapWidth(img), img.naturalWidth, img.naturalHeight));
  };

  const className = [props.className, lockWidth ? 'chat-image-tall' : undefined].filter(Boolean).join(' ');

  return (
    <img
      {...props}
      alt={props.alt ?? ''}
      className={className || undefined}
      onClick={openOriginal}
      onLoad={onLoad}
      // Reachable by keyboard without claiming to be a button: the image is
      // inside a text bubble, and a nested interactive control is what screen
      // readers trip on. `title` advertises the affordance on hover.
      role="link"
      tabIndex={0}
      title={props.alt || undefined}
      onKeyDown={(e) => {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault();
          openOriginal();
        }
      }}
    />
  );
}

// Copy-to-clipboard for the install commands. Falls back to a hidden textarea
// when the async clipboard API is unavailable (file:// and older Safari).
document.querySelectorAll('.cmd').forEach((block) => {
  const button = block.querySelector('.copy');
  const text = block.dataset.copy;
  if (!button || !text) return;

  button.addEventListener('click', async () => {
    try {
      if (navigator.clipboard) {
        await navigator.clipboard.writeText(text);
      } else {
        const scratch = document.createElement('textarea');
        scratch.value = text;
        scratch.setAttribute('readonly', '');
        scratch.style.position = 'fixed';
        scratch.style.opacity = '0';
        document.body.appendChild(scratch);
        scratch.select();
        document.execCommand('copy');
        document.body.removeChild(scratch);
      }
      button.textContent = 'Copied';
      button.classList.add('done');
    } catch {
      button.textContent = 'Press ⌘C';
    }
    setTimeout(() => {
      button.textContent = 'Copy';
      button.classList.remove('done');
    }, 1800);
  });
});

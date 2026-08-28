const $ = id => document.getElementById(id);
const id = location.pathname.split('/').filter(Boolean).pop();
const token = new URLSearchParams(location.search).get('token');
const labels = {
  awaiting_payment: 'Awaiting payment',
  partially_paid: 'Partially paid',
  paid: 'Payment detected',
  settled: 'Paid',
  returned: 'Sent to refund address',
  expired: 'Expired',
  needs_attention: 'Needs attention',
};
const terminal = new Set(['settled', 'returned', 'needs_attention']);

let invoice;
let timer;
let poll;
let serverOffset = 0;

function reveal(name) {
  for (const section of ['loading', 'error', 'payment']) {
    $(section).classList.toggle('hidden', section !== name);
  }
}

function fail() {
  clearInterval(timer);
  clearTimeout(poll);
  reveal('error');
}

function compact(amount) {
  return amount.replace(/(\.\d*?[1-9])0+$|\.0+$/, '$1');
}

function serverNow() {
  return Math.floor(Date.now() / 1000 + serverOffset);
}

function countdown() {
  const left = Math.floor(Date.parse(invoice.expires_at) / 1000) - serverNow();
  const countdown = $('countdown');
  if (!invoice.payable || left < 0) {
    countdown.textContent = ['awaiting_payment', 'partially_paid'].includes(invoice.status)
      ? 'Deadline passed · awaiting final status'
      : 'Deadline passed';
    return;
  }
  const days = Math.floor(left / 86400);
  const hours = Math.floor(left % 86400 / 3600);
  const minutes = Math.floor(left % 3600 / 60);
  const seconds = left % 60;
  countdown.textContent = (days ? `${days}d ` : '')
    + `${String(hours).padStart(2, '0')}:${String(minutes).padStart(2, '0')}:${String(seconds).padStart(2, '0')} left`;
}

function renderPaymentActions(data) {
  const qr = $('qr');
  const wallet = $('wallet');
  const copy = $('copy');
  $('payment-qr').classList.toggle('hidden', !data.payable);
  wallet.classList.toggle('hidden', !data.payable);
  copy.disabled = !data.payable;

  if (data.payable && data.payment_uri) {
    wallet.href = data.payment_uri;
    wallet.removeAttribute('aria-disabled');
    qr.src = `/v1/payer/payments/${encodeURIComponent(id)}/qr?token=${encodeURIComponent(token)}`;
  } else {
    wallet.removeAttribute('href');
    wallet.setAttribute('aria-disabled', 'true');
    qr.removeAttribute('src');
  }
}

function renderResolution(data) {
  const panel = $('settled');
  const resolved = data.status === 'settled' || data.status === 'returned' || data.status === 'needs_attention';
  panel.classList.toggle('hidden', !resolved);
  if (!resolved) return;

  const recovered = data.status === 'returned';
  const attention = data.status === 'needs_attention';
  $('resolution-title').textContent = attention ? 'Payout needs attention' : recovered ? 'Funds sent to refund address' : 'Payment complete';
  $('resolution-copy').textContent = attention
    ? data.payer_message
    : recovered
    ? 'This payment was not sent to the beneficiary. Contact the merchant for help.'
    : 'The payment has settled on-chain.';
  $('resolution-icon').textContent = attention ? '!' : recovered ? '↩' : '✓';
  panel.classList.toggle('problem', recovered || attention);
  const transaction = $('transaction');
  transaction.classList.toggle('hidden', !data.settlement_explorer_url);
  if (data.settlement_explorer_url) transaction.href = data.settlement_explorer_url;
}

function paint(data) {
  invoice = data;
  serverOffset = Number(data.server_timestamp) - Date.now() / 1000;
  reveal('payment');
  $('amount').textContent = compact(data.remaining);
  $('address').textContent = data.address;
  $('address').href = data.address_explorer_url || '#';
  $('address').target = data.address_explorer_url ? '_blank' : '';
  $('network').textContent = `${data.chain.name} (chain ${data.chain.id}) only`;
  $('token').textContent = data.token.address;

  const pill = $('status-pill');
  pill.textContent = !data.payable && ['awaiting_payment', 'partially_paid'].includes(data.status)
    ? 'Deadline passed'
    : labels[data.status] || data.status;
  pill.className = `pill ${data.status === 'settled' ? 'success' : ['returned', 'expired', 'needs_attention'].includes(data.status) || !data.payable && ['awaiting_payment', 'partially_paid'].includes(data.status) ? 'problem' : ''}`;

  $('progress').textContent = `${data.received} of ${data.amount} USDC finalized · ${data.remaining} remaining`;
  $('progress').classList.toggle('hidden', data.received_base_units === '0');
  renderPaymentActions(data);
  renderResolution(data);
  clearInterval(timer);
  countdown();
  if (data.payable) timer = setInterval(countdown, 1000);
}

function nextPoll(data) {
  if (terminal.has(data.status)) return null;
  if (data.status === 'expired') {
    return data.received_base_units === '0' ? null : 30000;
  }
  return 4000;
}

async function refresh() {
  try {
    const response = await fetch(
      `/v1/payer/payments/${encodeURIComponent(id)}?token=${encodeURIComponent(token || '')}`,
      { cache: 'no-store' },
    );
    if (!response.ok) throw new Error('payment read failed');
    const data = await response.json();
    paint(data);
    const delay = nextPoll(data);
    if (delay) poll = setTimeout(refresh, delay);
  } catch {
    fail();
  }
}

$('copy').addEventListener('click', async () => {
  if (!invoice?.payable) return;
  try {
    await navigator.clipboard.writeText(invoice.address);
    $('copy').textContent = 'Copied';
    setTimeout(() => $('copy').textContent = 'Copy', 1600);
  } catch {
    $('error-copy').textContent = 'Copy the address manually.';
  }
});

if (!id || !token) fail();
else refresh();

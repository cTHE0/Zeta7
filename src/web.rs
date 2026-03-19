use std::sync::Arc;
use std::convert::Infallible;
use std::pin::Pin;
use axum::{
    Router,
    routing::{get, post},
    extract::State,
    response::{Html, Json, sse::{Sse, Event, KeepAlive}},
};
use futures_util::stream::Stream;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt as _;
use serde::{Deserialize, Serialize};
use crate::app_state::{AppState, ChatMessage, UiCommand};

// ────────────────────────────────────────────────────────────
// Interface embarquée (page HTML complète)
// ────────────────────────────────────────────────────────────
const INDEX_HTML: &str = r#"<!DOCTYPE html>
<html lang="fr">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>Zeta Network</title>
<style>
:root{
  --bg:#ffffff;--surface:#f8f8f8;--border:#ebebeb;
  --text:#111111;--text2:#666666;--text3:#aaaaaa;
  --green:#16a34a;--green-bg:#f0fdf4;--green-bd:#bbf7d0;
  --red:#dc2626;--red-bg:#fef2f2;--red-bd:#fecaca;
  --r:10px;--rs:6px;
}
*{margin:0;padding:0;box-sizing:border-box}
body{font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',system-ui,sans-serif;
  background:var(--bg);color:var(--text);height:100vh;
  display:flex;flex-direction:column;overflow:hidden;font-size:14px;line-height:1.5}

/* ── Header ── */
header{height:52px;border-bottom:1px solid var(--border);
  display:flex;align-items:center;padding:0 20px;gap:12px;flex-shrink:0}
.logo{display:flex;align-items:center;gap:9px;font-weight:700;font-size:15px;letter-spacing:-.3px}
.logo-box{width:28px;height:28px;background:var(--text);border-radius:7px;
  display:flex;align-items:center;justify-content:center;
  color:#fff;font-size:13px;font-weight:800}
.node-chip{font-size:12px;color:var(--text2);background:var(--surface);
  border:1px solid var(--border);padding:3px 10px;border-radius:999px;font-family:monospace}
.hspacer{flex:1}
.status{display:flex;align-items:center;gap:6px;font-size:12px;color:var(--text3)}
.dot{width:7px;height:7px;border-radius:50%;background:#22c55e;
  box-shadow:0 0 0 3px rgba(34,197,94,.15)}

/* ── Layout ── */
.layout{display:flex;flex:1;overflow:hidden}

/* ── Sidebar ── */
.sidebar{width:256px;flex-shrink:0;border-right:1px solid var(--border);
  overflow-y:auto;padding:14px;display:flex;flex-direction:column;gap:10px}
.card{border:1px solid var(--border);border-radius:var(--r);padding:14px}
.card-hd{font-size:10px;font-weight:600;letter-spacing:.6px;text-transform:uppercase;
  color:var(--text3);margin-bottom:10px}
.field{margin-bottom:8px}.field:last-child{margin-bottom:0}
.fl{font-size:11px;color:var(--text3);margin-bottom:2px}
.fv{font-size:12px;color:var(--text);font-weight:500;word-break:break-all;
  font-family:'SF Mono','Fira Code',monospace}
.bal{display:flex;align-items:baseline;gap:4px;margin-bottom:8px}
.bal-n{font-size:28px;font-weight:700;letter-spacing:-.5px;line-height:1}
.bal-u{font-size:13px;color:var(--text2)}
.fgrp{margin-bottom:8px}.fgrp:last-child{margin-bottom:0}
input[type=text],input[type=number]{width:100%;padding:8px 10px;
  border:1px solid var(--border);border-radius:var(--rs);font-size:13px;
  outline:none;background:var(--bg);color:var(--text);font-family:inherit;
  transition:border-color .15s,box-shadow .15s}
input:focus{border-color:#888;box-shadow:0 0 0 3px rgba(0,0,0,.05)}
input::placeholder{color:var(--text3)}
.btn{display:flex;align-items:center;justify-content:center;gap:6px;
  padding:9px 14px;background:var(--text);color:#fff;border:none;
  border-radius:var(--rs);font-size:13px;font-weight:500;cursor:pointer;
  width:100%;font-family:inherit;transition:opacity .15s,transform .1s}
.btn:hover{opacity:.85}.btn:active{opacity:.7;transform:scale(.99)}

/* ── Chat ── */
.chat{flex:1;display:flex;flex-direction:column;overflow:hidden}
.messages{flex:1;overflow-y:auto;padding:20px;
  display:flex;flex-direction:column;gap:2px}
.empty{flex:1;display:flex;flex-direction:column;align-items:center;
  justify-content:center;color:var(--text3);gap:8px;text-align:center;padding:40px}
.empty-ico{font-size:38px;opacity:.25}
.empty-t{font-size:15px;font-weight:500;color:var(--text2)}
.empty-s{font-size:13px}

/* ── Message ── */
.mg{display:flex;gap:10px;padding:3px 0;margin-bottom:6px}
.mg:hover .mt{opacity:1}
.mg.sent{flex-direction:row-reverse}
.mg.sent .mb{text-align:right}
.av{width:32px;height:32px;border-radius:8px;display:flex;align-items:center;
  justify-content:center;font-weight:700;font-size:11px;flex-shrink:0;margin-top:2px}
.mb{flex:1;min-width:0}
.mm{display:flex;align-items:baseline;gap:7px;margin-bottom:2px}
.mg.sent .mm{flex-direction:row-reverse}
.ma{font-weight:600;font-size:13.5px}
.mfp{font-size:11px;color:var(--text3);font-family:monospace}
.mt{font-size:11px;color:var(--text3);margin-left:auto;opacity:0;transition:opacity .15s}
.mg.sent .mt{margin-left:0;margin-right:auto}
.mx{font-size:14px;color:var(--text);line-height:1.55;word-break:break-word}

/* ── Payment events ── */
.pev{display:flex;align-items:center;gap:8px;padding:3px 0;margin-bottom:6px}
.pill{display:inline-flex;align-items:center;gap:6px;padding:5px 13px;
  border-radius:999px;font-size:13px;font-weight:500;border:1px solid}
.pin{background:var(--green-bg);color:var(--green);border-color:var(--green-bd)}
.pout{background:var(--red-bg);color:var(--red);border-color:var(--red-bd)}
.pack{background:var(--surface);color:var(--text2);border-color:var(--border)}
.pt{font-size:11px;color:var(--text3);margin-left:4px}

/* ── Input bar ── */
.ibar{border-top:1px solid var(--border);padding:12px 20px;
  display:flex;gap:8px;flex-shrink:0}
.ibar input{flex:1;padding:10px 14px;border-radius:999px;
  background:var(--surface);border:1px solid var(--border)}
.ibar input:focus{background:var(--bg)}
.sbtn{width:40px;height:40px;padding:0;border-radius:50%;flex-shrink:0;font-size:16px}

/* ── Toasts ── */
#toasts{position:fixed;bottom:76px;right:20px;display:flex;
  flex-direction:column;gap:8px;z-index:100;pointer-events:none}
.toast{background:var(--text);color:#fff;padding:10px 16px;border-radius:var(--r);
  font-size:13px;font-weight:500;box-shadow:0 4px 20px rgba(0,0,0,.18);
  animation:sin .3s ease,sout .3s ease 2.7s forwards;max-width:300px}
.toast.ok{background:#16a34a}
.toast.err{background:#dc2626}
@keyframes sin{from{transform:translateX(16px);opacity:0}to{transform:translateX(0);opacity:1}}
@keyframes sout{from{opacity:1}to{opacity:0}}
::-webkit-scrollbar{width:4px}
::-webkit-scrollbar-track{background:transparent}
::-webkit-scrollbar-thumb{background:var(--border);border-radius:4px}
</style>
</head>
<body>

<header>
  <div class="logo">
    <div class="logo-box">Z</div>
    Zeta
  </div>
  <span class="node-chip" id="h-id">...</span>
  <div class="hspacer"></div>
  <div class="status"><div class="dot"></div><span>Connecté</span></div>
</header>

<div class="layout">
  <!-- Sidebar -->
  <aside class="sidebar">
    <div class="card">
      <div class="card-hd">Identité</div>
      <div class="field">
        <div class="fl">Peer ID</div>
        <div class="fv" id="i-id">—</div>
      </div>
      <div class="field">
        <div class="fl">Empreinte Ed25519</div>
        <div class="fv" id="i-fp">—</div>
      </div>
    </div>

    <div class="card">
      <div class="card-hd">Portefeuille</div>
      <div class="bal">
        <span class="bal-n" id="balance">0</span>
        <span class="bal-u">sats</span>
      </div>
      <div class="field">
        <div class="fl">Adresse Bitcoin</div>
        <div class="fv" id="i-btc" style="font-size:10px">—</div>
      </div>
    </div>

    <div class="card">
      <div class="card-hd">Envoyer des sats</div>
      <div class="fgrp">
        <input type="text" id="pay-to" placeholder="Adresse Bitcoin du destinataire"/>
      </div>
      <div class="fgrp">
        <input type="number" id="pay-amt" placeholder="Montant (sats)" min="1"/>
      </div>
      <button class="btn" onclick="sendPayment()">Envoyer ↗</button>
    </div>
  </aside>

  <!-- Chat -->
  <div class="chat">
    <div class="messages" id="feed">
      <div class="empty" id="empty">
        <div class="empty-ico">◌</div>
        <div class="empty-t">Aucun message</div>
        <div class="empty-s">Les messages du réseau apparaîtront ici</div>
      </div>
    </div>
    <div class="ibar">
      <input type="text" id="msg-in"
        placeholder="Écrire un message sur le réseau…"
        onkeydown="if(event.key==='Enter'&&!event.shiftKey){sendMsg();event.preventDefault()}"/>
      <button class="btn sbtn" onclick="sendMsg()">↑</button>
    </div>
  </div>
</div>

<div id="toasts"></div>

<script>
'use strict';
let myId='';

/* ── API ── */
async function loadInfo(){
  try{
    const d=await(await fetch('/api/info')).json();
    myId=d.node_id;
    document.getElementById('h-id').textContent=d.node_id;
    document.getElementById('i-id').textContent=d.node_id;
    document.getElementById('i-fp').textContent=d.fingerprint;
    document.getElementById('i-btc').textContent=d.btc_address;
    document.getElementById('balance').textContent=fmtSats(d.balance);
  }catch(e){console.error(e)}
}

async function loadMessages(){
  try{
    const msgs=await(await fetch('/api/messages')).json();
    msgs.forEach(m=>appendMsg(m,false));
    scrollBot();
  }catch(e){console.error(e)}
}

async function sendMsg(){
  const el=document.getElementById('msg-in');
  const text=el.value.trim();if(!text)return;el.value='';
  try{await fetch('/api/send',{method:'POST',
    headers:{'Content-Type':'application/json'},
    body:JSON.stringify({text})});}catch(e){console.error(e)}
}

async function sendPayment(){
  const dst=document.getElementById('pay-to').value.trim();
  const amt=parseInt(document.getElementById('pay-amt').value||'0');
  if(!dst){toast('Entrez un destinataire','err');return}
  if(!amt||amt<=0){toast('Montant invalide','err');return}
  try{
    const r=await fetch('/api/pay',{method:'POST',
      headers:{'Content-Type':'application/json'},
      body:JSON.stringify({dst_id:dst,amount:amt})});
    const d=await r.json();
    if(d.ok){
      document.getElementById('pay-to').value='';
      document.getElementById('pay-amt').value='';
      toast(`${fmtSats(amt)} sats envoyés à ${dst}`,'ok');
    }else{toast(d.error||'Erreur de paiement','err')}
  }catch(e){toast('Erreur réseau','err')}
}

/* ── Rendering ── */
function appendMsg(m,scroll=true){
  const feed=document.getElementById('feed');
  document.getElementById('empty').style.display='none';
  const t=fmtTime(m.timestamp);
  let el=document.createElement('div');

  if(m.msg_type==='message'){
    const c=avatarColor(m.from_id);
    const ini=(m.from_id||'?').substring(0,2).toUpperCase();
    const isSent=m.from_id.includes('(vous)');
    el.className=isSent?'mg sent':'mg';
    el.innerHTML=`
      <div class="av" style="background:${c.bg};color:${c.fg}">${ini}</div>
      <div class="mb">
        <div class="mm">
          <span class="ma">${esc(m.from_id)}</span>
          <span class="mfp">${esc(m.fingerprint)}</span>
          <span class="mt">${t}</span>
        </div>
        <div class="mx">${esc(m.text)}</div>
      </div>`;
  } else {
    el.className='pev';
    let pc='pack',html='';
    if(m.msg_type==='payment_received'){
      pc='pin';
      html=`<strong>+${esc(m.text)} sats</strong> reçus de <strong>${esc(m.from_id)}</strong>`;
    } else if(m.msg_type==='payment_sent'){
      pc='pout';
      // Le format du text est "montant → adresse_btc"
      const parts=m.text.split('→').map(s=>s.trim());
      if(parts.length===2){
        html=`<strong>−${esc(parts[0])} sats</strong> envoyés à <strong>${esc(parts[1])}</strong>`;
      }else{
        html=`<strong>−${esc(m.text)} sats</strong> envoyés`;
      }
    } else if(m.msg_type==='payment_ack'){
      html=`✓ Paiement confirmé par <strong>${esc(m.from_id)}</strong>`;
    }
    el.innerHTML=`<span class="pill ${pc}">${html}</span><span class="pt">${t}</span>`;
  }
  feed.appendChild(el);
  if(scroll)scrollBot();
}

function scrollBot(){
  const f=document.getElementById('feed');f.scrollTop=f.scrollHeight;
}

/* ── SSE ── */
function connectSSE(){
  const es=new EventSource('/api/events');
  es.onmessage=e=>{
    try{
      const ev=JSON.parse(e.data);
      if(ev.event_type==='message'){
        const m=JSON.parse(ev.data);
        appendMsg(m,true);
        if(m.from_id!==myId) toast(`${m.from_id}: ${m.text.substring(0,50)}`,'ok');
      } else if(ev.event_type==='payment'){
        const m=JSON.parse(ev.data);
        appendMsg(m,true);
        if(m.msg_type==='payment_received')
          toast(`+${fmtSats(+m.text)} sats reçus de ${m.from_id}`,'ok');
      } else if(ev.event_type==='wallet_update'){
        const w=JSON.parse(ev.data);
        document.getElementById('balance').textContent=fmtSats(w.balance);
      }
    }catch(e){console.error('SSE',e)}
  };
  es.onerror=()=>{es.close();setTimeout(connectSSE,2000)};
}

/* ── Helpers ── */
function fmtSats(n){return Number(n).toLocaleString('fr-FR')}
function fmtTime(ts){
  return new Date(ts*1000).toLocaleTimeString([],{hour:'2-digit',minute:'2-digit'})
}
function avatarColor(id){
  let h=5381;
  for(let i=0;i<id.length;i++) h=((h<<5)+h)+id.charCodeAt(i);
  const hue=Math.abs(h)%360;
  return{bg:`hsl(${hue},28%,92%)`,fg:`hsl(${hue},40%,28%)`};
}
function esc(s){
  return String(s||'')
    .replace(/&/g,'&amp;').replace(/</g,'&lt;')
    .replace(/>/g,'&gt;').replace(/"/g,'&quot;');
}
function toast(msg,type='ok'){
  const c=document.getElementById('toasts');
  const el=document.createElement('div');
  el.className=`toast ${type}`;el.textContent=msg;
  c.appendChild(el);setTimeout(()=>el.remove(),3200);
}

/* ── Init ── */
loadInfo();loadMessages();connectSSE();

// Rafraîchissement automatique toutes les 5 secondes
setInterval(async()=>{
  try{
    const d=await(await fetch('/api/info')).json();
    document.getElementById('balance').textContent=fmtSats(d.balance);
  }catch(e){console.error(e)}
},5000);
</script>
</body></html>"#;

// ────────────────────────────────────────────────────────────
// Types de requête / réponse
// ────────────────────────────────────────────────────────────
#[derive(Serialize)]
struct NodeInfo {
    node_id: String,
    fingerprint: String,
    btc_address: String,
    balance: u64,
}

#[derive(Deserialize)]
struct SendMessageReq {
    text: String,
}

#[derive(Deserialize)]
struct PayReq {
    dst_id: String,
    amount: u64,
}

#[derive(Serialize)]
struct PayResponse {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

// ────────────────────────────────────────────────────────────
// Handlers
// ────────────────────────────────────────────────────────────
async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

async fn api_info(State(state): State<Arc<AppState>>) -> Json<NodeInfo> {
    let balance = state.wallet.lock().await.balance;
    Json(NodeInfo {
        node_id: state.node_id.clone(),
        fingerprint: state.fingerprint.clone(),
        btc_address: state.btc_address.clone(),
        balance,
    })
}

async fn api_messages(State(state): State<Arc<AppState>>) -> Json<Vec<ChatMessage>> {
    let msgs = state.messages.read().await;
    Json(msgs.clone())
}

async fn api_send(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SendMessageReq>,
) -> Json<serde_json::Value> {
    if body.text.trim().is_empty() {
        return Json(serde_json::json!({"ok": false, "error": "Message vide"}));
    }
    let _ = state.cmd_tx.send(UiCommand::SendMessage(body.text)).await;
    Json(serde_json::json!({"ok": true}))
}

async fn api_pay(
    State(state): State<Arc<AppState>>,
    Json(body): Json<PayReq>,
) -> Json<PayResponse> {
    if body.amount == 0 {
        return Json(PayResponse {
            ok: false,
            error: Some("Montant invalide".to_string()),
        });
    }
    let _ = state.cmd_tx.send(UiCommand::Pay {
        dst_id: body.dst_id,
        amount: body.amount,
    }).await;
    Json(PayResponse { ok: true, error: None })
}

type SseStream = Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>>;

async fn api_events(State(state): State<Arc<AppState>>) -> Sse<SseStream> {
    let rx = state.events_tx.subscribe();
    let stream: SseStream = Box::pin(
        BroadcastStream::new(rx).filter_map(|r| {
            r.ok().map(|ev| {
                let data = serde_json::to_string(&ev).unwrap_or_default();
                Ok::<Event, Infallible>(Event::default().data(data))
            })
        })
    );
    Sse::new(stream).keep_alive(KeepAlive::default())
}
// ────────────────────────────────────────────────────────────
// Point d'entrée du serveur
// ────────────────────────────────────────────────────────────
pub async fn start_server(state: Arc<AppState>, port: u16) {
    let app = Router::new()
        .route("/", get(index))
        .route("/api/info", get(api_info))
        .route("/api/messages", get(api_messages))
        .route("/api/send", post(api_send))
        .route("/api/pay", post(api_pay))
        .route("/api/events", get(api_events))
        .with_state(state);

    let addr = format!("127.0.0.1:{}", port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .expect("Impossible de lier le serveur web");
    println!("╔══════════════════════════════════════════╗");
    println!("║  Interface web : http://{}  ║", addr);
    println!("╚══════════════════════════════════════════╝");
    axum::serve(listener, app).await.expect("Erreur serveur web");
}

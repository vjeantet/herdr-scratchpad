#!/usr/bin/env bash
# Ouvre / focalise / ferme le pane scratchpad, docké en bas.
#
# Ce script n'est qu'un enchaînement de commandes herdr : TOUTES les décisions
# viennent du binaire du plugin, en modes stdin->stdout (--launch-decision,
# --focused-pane, --open-plan), pour qu'elles soient testables en Rust plutôt
# qu'ici.
set -u

script_dir="$(cd "$(dirname "$0")" && pwd)"
herdr_bin="${HERDR_BIN_PATH:-herdr}"
bin="$script_dir/../target/release/herdr-scratchpad"

# Pas de binaire construit : repli déclaratif. Le pane s'ouvrira à droite en
# 50/50 (le manifeste ne sait pas exprimer un dock bas, cf. DESIGN §3), ce qui
# reste utilisable en attendant un `cargo build --release`.
if [ ! -x "$bin" ]; then
  exec "$herdr_bin" plugin pane open \
    --plugin herdr-scratchpad \
    --entrypoint scratchpad \
    --placement split \
    --direction down \
    --focus
fi

# Déplace le focus hors du pane avant de le fermer.
#
# `pane close` tue le processus sans signal : la temporisation de sauvegarde
# emporterait les dernières frappes. Perdre le focus déclenche une sauvegarde
# immédiate dans la TUI (herdr transmet l'événement, cf. pane.rs
# try_send_focus_event), et 0,4 s suffisent largement à son cycle de boucle.
retire_pane() {
  "$herdr_bin" pane focus --direction up >/dev/null 2>&1 || true
  sleep 0.4
  "$herdr_bin" pane close "$1" >/dev/null 2>&1 || true
}

open_pane() {
  # `pane list` sans --workspace, volontairement : la liste globale est la
  # seule où figure le pane réellement focalisé. La scoper avec l'id de
  # workspace du shell lanceur (figé au spawn) peut faire disparaître le pane
  # focalisé et dégrader la décision en OPEN, donc en doublon.
  fp="$(printf '%s' "$1" | "$bin" --focused-pane 2>/dev/null || true)"
  fid="${fp%%	*}"
  fcwd="${fp#*	}"

  if [ -z "$fid" ]; then
    exec "$herdr_bin" plugin pane open \
      --plugin herdr-scratchpad --entrypoint scratchpad \
      --placement split --direction down --focus
  fi

  target="$fid"
  ratio="0.70"
  plan="$("$herdr_bin" pane layout --pane "$fid" 2>/dev/null | "$bin" --open-plan 2>/dev/null || true)"
  if [ -n "$plan" ]; then
    target="${plan%%	*}"
    ratio="${plan#*	}"
  fi

  # --ratio est la part de la CIBLE ; le nouveau pane reçoit le reste. Scinder
  # vers le bas le pane le plus bas le pose contre le bord inférieur, sans
  # échange de panes.
  #
  # --env est indispensable : un pane créé par `pane split` n'hérite PAS de
  # HERDR_PLUGIN_STATE_DIR (seules les actions le reçoivent), et sans lui la
  # TUI écrirait son état ailleurs que le reste du plugin.
  out="$("$herdr_bin" pane split "$target" \
    --direction down --ratio "$ratio" \
    ${fcwd:+--cwd "$fcwd"} \
    ${HERDR_PLUGIN_STATE_DIR:+--env "HERDR_PLUGIN_STATE_DIR=$HERDR_PLUGIN_STATE_DIR"} \
    --no-focus 2>/dev/null || true)"
  np="$(printf '%s' "$out" | sed -n 's/.*"pane_id":"\([^"]*\)".*/\1/p' | head -n1)"
  [ -n "$np" ] || exit 1

  # L'estampille tombe ENTRE le split et le run, donc avant que la TUI
  # démarre : un pane neuf n'est jamais observable sans jeton, sinon un second
  # toggle pendant le démarrage le prendrait pour un cadavre et le remplacerait
  # en boucle.
  "$bin" --stamp "$np" >/dev/null 2>&1 || true

  # `pane run` ne lance pas un processus : il TAPE la commande dans le shell du
  # pane puis Entrée. `exec` remplace donc le shell, et le pane meurt avec la
  # TUI.
  "$herdr_bin" pane run "$np" "exec \"$bin\""
  "$herdr_bin" pane rename "$np" "Scratchpad" >/dev/null 2>&1 || true

  # herdr n'a pas de « focaliser tel pane » : le cycle zoom on/off y arrive.
  "$herdr_bin" pane zoom "$np" --on >/dev/null 2>&1 || true
  exec "$herdr_bin" pane zoom "$np" --off
}

panes="$("$herdr_bin" pane list 2>/dev/null || true)"

decision="OPEN"
if [ -n "$panes" ]; then
  decision="$(printf '%s' "$panes" | "$bin" --launch-decision 2>/dev/null || echo OPEN)"
fi

case "$decision" in
  "FOCUS "*)
    pid="${decision#FOCUS }"
    "$herdr_bin" pane zoom "$pid" --on >/dev/null 2>&1 || true
    exec "$herdr_bin" pane zoom "$pid" --off
    ;;
  "CLOSE "*)
    retire_pane "${decision#CLOSE }"
    exit 0
    ;;
  "REPLACE "*)
    retire_pane "${decision#REPLACE }"
    # Re-lister est obligatoire : un instantané de `pane list` périme dès
    # qu'on ferme un pane, et le cadavre pouvait être le pane focalisé — un
    # instantané périmé fait échouer `pane layout`/`pane split` en
    # pane_not_found.
    panes="$("$herdr_bin" pane list 2>/dev/null || true)"
    open_pane "$panes"
    ;;
  *)
    open_pane "$panes"
    ;;
esac

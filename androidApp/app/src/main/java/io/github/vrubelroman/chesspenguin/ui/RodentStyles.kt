package io.github.vrubelroman.chesspenguin.ui

import io.github.vrubelroman.chesspenguin.R

/// Rodent IV's characters in the app's language (android-rodent.md §3.3): their names and
/// what they do, as the desktop app says them (apps/desktop/src/i18n.rs), by the ids of
/// the shared list (`play::rodent::STYLES`, given by `rodentStyles()`).
val RODENT_TEXTS: Map<String, Pair<Int, Int>> = mapOf(
    "iv_amanda" to (R.string.rodent_iv_amanda to R.string.rodent_iv_amanda_about),
    "iv_cloe" to (R.string.rodent_iv_cloe to R.string.rodent_iv_cloe_about),
    "iv_deborah" to (R.string.rodent_iv_deborah to R.string.rodent_iv_deborah_about),
    "iv_defender" to (R.string.rodent_iv_defender to R.string.rodent_iv_defender_about),
    "iv_dynamic" to (R.string.rodent_iv_dynamic to R.string.rodent_iv_dynamic_about),
    "iv_grumpy" to (R.string.rodent_iv_grumpy to R.string.rodent_iv_grumpy_about),
    "iv_partisan" to (R.string.rodent_iv_partisan to R.string.rodent_iv_partisan_about),
    "iv_pawnsacker" to (R.string.rodent_iv_pawnsacker to R.string.rodent_iv_pawnsacker_about),
    "iv_pedrita" to (R.string.rodent_iv_pedrita to R.string.rodent_iv_pedrita_about),
    "iv_preston" to (R.string.rodent_iv_preston to R.string.rodent_iv_preston_about),
    "iv_simple" to (R.string.rodent_iv_simple to R.string.rodent_iv_simple_about),
    "iv_spitfire" to (R.string.rodent_iv_spitfire to R.string.rodent_iv_spitfire_about),
    "iv_strangler" to (R.string.rodent_iv_strangler to R.string.rodent_iv_strangler_about),
    "iv_alekhine" to (R.string.rodent_iv_alekhine to R.string.rodent_iv_alekhine_about),
    "iv_anand" to (R.string.rodent_iv_anand to R.string.rodent_iv_anand_about),
    "iv_anderssen" to (R.string.rodent_iv_anderssen to R.string.rodent_iv_anderssen_about),
    "iv_bosboom" to (R.string.rodent_iv_bosboom to R.string.rodent_iv_bosboom_about),
    "iv_botvinnik" to (R.string.rodent_iv_botvinnik to R.string.rodent_iv_botvinnik_about),
    "iv_fischer" to (R.string.rodent_iv_fischer to R.string.rodent_iv_fischer_about),
    "iv_karpov" to (R.string.rodent_iv_karpov to R.string.rodent_iv_karpov_about),
    "iv_kasparov" to (R.string.rodent_iv_kasparov to R.string.rodent_iv_kasparov_about),
    "iv_kortchnoi" to (R.string.rodent_iv_kortchnoi to R.string.rodent_iv_kortchnoi_about),
    "iv_larsen" to (R.string.rodent_iv_larsen to R.string.rodent_iv_larsen_about),
    "iv_lasker" to (R.string.rodent_iv_lasker to R.string.rodent_iv_lasker_about),
    "iv_marshall" to (R.string.rodent_iv_marshall to R.string.rodent_iv_marshall_about),
    "iv_morphy" to (R.string.rodent_iv_morphy to R.string.rodent_iv_morphy_about),
    "iv_nimzowitsch" to (R.string.rodent_iv_nimzowitsch to R.string.rodent_iv_nimzowitsch_about),
    "iv_petrosian" to (R.string.rodent_iv_petrosian to R.string.rodent_iv_petrosian_about),
    "iv_reti" to (R.string.rodent_iv_reti to R.string.rodent_iv_reti_about),
    "iv_rubinstein" to (R.string.rodent_iv_rubinstein to R.string.rodent_iv_rubinstein_about),
    "iv_spassky" to (R.string.rodent_iv_spassky to R.string.rodent_iv_spassky_about),
    "iv_steinitz" to (R.string.rodent_iv_steinitz to R.string.rodent_iv_steinitz_about),
    "iv_tal" to (R.string.rodent_iv_tal to R.string.rodent_iv_tal_about),
    "iv_tarrasch" to (R.string.rodent_iv_tarrasch to R.string.rodent_iv_tarrasch_about),
    "iv_topalov" to (R.string.rodent_iv_topalov to R.string.rodent_iv_topalov_about),
)

/// The character suggested first (android-rodent.md §3.4), and the one an unknown id
/// (of a later version) falls back to.
const val DEFAULT_RODENT_STYLE = "iv_tal"

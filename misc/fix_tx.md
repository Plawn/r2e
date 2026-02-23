# Plan: audit #[managed] / Tx

## Probleme observe
- #[managed] calcule le succes uniquement si le type de retour se termine par "Result".
  Les alias courants (ApiResult, JsonResult, StatusResult) ne sont pas detectes.
  Impact: un handler peut retourner Err, mais __success reste true, ce qui provoque un commit Tx.
- Documentation incoherente sur les imports: l'exemple #[managed] reference Tx/HasPool depuis r2e_core,
  alors que ces types viennent de r2e_data_sqlx (ou r2e::prelude avec feature data-sqlx).
- Exemple d'usage du Tx peu ergonomique: "&mut **tx" alors que Tx expose as_mut() et DerefMut.

## Racine du probleme
- is_result_type dans r2e-macros ne reconnait que le segment ident "Result".
  Les alias de type n'apparaissent pas comme "Result" dans l'AST et sont traites comme non-Result.
- Le code genere applique alors "__success = true" pour tout type non-Result.

## Plan de remediation (court terme)
1) Etendre la detection de "Result-like" dans r2e-macros
   - Modifier is_result_type pour reconnaitre: Result, ApiResult, JsonResult, StatusResult.
   - Accepter les formes qualifiees (ex: r2e_core::types::JsonResult).
   - Ajouter un commentaire court pour documenter la liste.

2) Ajouter une verification minimale
   - Test macro ou test compile qui verifie que #[managed] + JsonResult genere "__result.is_ok()".
   - Si aucun harness existant, ajouter un test unitaire dans r2e-macros qui parse un handler
     et valide le token genere (snapshot basique).

3) Corriger la documentation
   - r2e-macros/src/lib.rs: utiliser r2e_data_sqlx::{Tx, HasPool} ou r2e::prelude::*.
   - Remplacer "&mut **tx" par "tx.as_mut()" dans l'exemple.
   - Option: README "Managed resources" avec un exemple JsonResult pour montrer le bon usage.

## Plan de remediation (moyen terme, optionnel)
- Mode strict: si un handler utilise #[managed], exiger un type Result (sinon erreur compile).
  Cela elimine les commits implicites sur erreurs.
- Ou: ajouter un attribut explicite pour controler le succes, ex:
  #[managed(result = "result")] (par defaut) ou #[managed(result = "always_ok")].
  A etudier selon la complexite macro et l'ergonomie souhaitee.

## Verification
- cargo check -p r2e-macros
- cargo check -p r2e-core
- Compiler l'exemple app pour verifier le cas JsonResult + #[managed]

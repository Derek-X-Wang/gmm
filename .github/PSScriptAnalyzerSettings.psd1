@{
    # CI scripts run with StrictMode and ErrorActionPreference=Stop. Warnings
    # from correctness rules therefore gate alongside errors; style-only rules
    # are omitted so this job stays about broken behavior, not formatting.
    Severity = @('Error', 'Warning')
    IncludeRules = @(
        'PSAvoidAssignmentToAutomaticVariable'
        'PSAvoidInvokingEmptyMembers'
        'PSAvoidMultipleTypeAttributes'
        'PSAvoidOverwritingBuiltInCmdlets'
        'PSAvoidReservedWordsAsFunctionNames'
        'PSAvoidUsingAllowUnencryptedAuthentication'
        'PSAvoidUsingBrokenHashAlgorithms'
        'PSAvoidUsingComputerNameHardcoded'
        'PSAvoidUsingConvertToSecureStringWithPlainText'
        'PSAvoidUsingEmptyCatchBlock'
        'PSAvoidUsingInvokeExpression'
        'PSAvoidUsingPlainTextForPassword'
        'PSAvoidUsingUsernameAndPasswordParams'
        'PSAvoidUsingWMICmdlet'
        'PSMisleadingBacktick'
        'PSPossibleIncorrectComparisonWithNull'
        'PSPossibleIncorrectUsageOfAssignmentOperator'
        'PSPossibleIncorrectUsageOfRedirectionOperator'
        'PSReservedCmdletChar'
        'PSReservedParams'
        'PSUseCmdletCorrectly'
        'PSUseDeclaredVarsMoreThanAssignments'
        'PSUseLiteralInitializerForHashtable'
        'PSUseUsingScopeModifierInNewRunspaces'
    )
}

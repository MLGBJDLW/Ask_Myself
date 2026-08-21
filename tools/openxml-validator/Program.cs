using System.Text.Json;
using DocumentFormat.OpenXml;
using DocumentFormat.OpenXml.Packaging;
using DocumentFormat.OpenXml.Validation;

const string EngineVersion = "3.5.1";

static object ErrorRecord(ValidationErrorInfo error) => new
{
    id = error.Id,
    errorType = error.ErrorType.ToString(),
    description = error.Description,
    part = error.Part?.Uri.ToString(),
    path = error.Path?.XPath,
    node = error.Node?.LocalName,
};

static List<object> Validate(string path)
{
    var validator = new OpenXmlValidator(FileFormatVersions.Microsoft365);
    var extension = Path.GetExtension(path).ToLowerInvariant();
    OpenXmlPackage package = extension switch
    {
        ".docx" or ".docm" or ".dotx" or ".dotm" =>
            WordprocessingDocument.Open(path, false),
        ".xlsx" or ".xlsm" or ".xltx" or ".xltm" =>
            SpreadsheetDocument.Open(path, false),
        ".pptx" or ".pptm" or ".potx" or ".potm" =>
            PresentationDocument.Open(path, false),
        _ => throw new ArgumentException($"Unsupported Office extension: {extension}"),
    };
    using (package)
    {
        return validator.Validate(package).Take(1001).Select(ErrorRecord).ToList();
    }
}

if (args.Length == 1 && args[0] == "--version")
{
    Console.WriteLine(JsonSerializer.Serialize(new
    {
        kind = "openXmlSdkValidatorStatus",
        engine = "DocumentFormat.OpenXml.OpenXmlValidator",
        engineVersion = EngineVersion,
        status = "ready",
    }));
    return 0;
}

if (args.Length != 1)
{
    Console.Error.WriteLine("usage: nexa-openxml-validator <office-package>");
    return 2;
}

try
{
    var path = Path.GetFullPath(args[0]);
    if (!File.Exists(path))
    {
        throw new FileNotFoundException("Office package not found", path);
    }
    var errors = Validate(path);
    var truncated = errors.Count > 1000;
    if (truncated)
    {
        errors.RemoveAt(errors.Count - 1);
    }
    Console.WriteLine(JsonSerializer.Serialize(new
    {
        kind = "openXmlSdkValidation",
        engine = "DocumentFormat.OpenXml.OpenXmlValidator",
        engineVersion = EngineVersion,
        fileFormatVersion = "Microsoft365",
        status = errors.Count == 0 ? "pass" : "fail",
        errorCount = errors.Count,
        truncated,
        errors,
    }));
    return errors.Count == 0 ? 0 : 1;
}
catch (Exception error)
{
    Console.WriteLine(JsonSerializer.Serialize(new
    {
        kind = "openXmlSdkValidation",
        engine = "DocumentFormat.OpenXml.OpenXmlValidator",
        engineVersion = EngineVersion,
        status = "error",
        error = $"{error.GetType().Name}: {error.Message}",
    }));
    return 2;
}

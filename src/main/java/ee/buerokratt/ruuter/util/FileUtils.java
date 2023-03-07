package ee.buerokratt.ruuter.util;

import org.ini4j.Ini;
import org.springframework.util.StringUtils;

import java.io.File;
import java.io.IOException;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.Map;
import java.util.stream.Collectors;

import static java.nio.file.Files.exists;
import static java.nio.file.Files.isDirectory;

public class FileUtils {
    public static final String SUFFIX_YAML = ".yaml";
    public static final String SUFFIX_YML = ".yml";

    private FileUtils() {
    }

    public static Path getFolderPath(String pathString) {
        if (StringUtils.hasLength(pathString)) {
            Path path = Paths.get(pathString);
            if (exists(path) && isDirectory(path)) {
                return path;
            }
        }
        throw new IllegalArgumentException("Failed to resolve directory: %s".formatted(pathString));
    }

    public static boolean isYmlFile(Path path) {
        String pathString = path.toString();
        return !isDirectory(path) && (pathString.endsWith(SUFFIX_YML) || pathString.endsWith(SUFFIX_YAML));
    }

    public static String getFileNameWithoutSuffix(Path path) {
        String nameWithSuffix = path.getFileName().toString();
        return nameWithSuffix.substring(0, nameWithSuffix.lastIndexOf('.'));
    }

    public static String getFileNameWithPathWithoutSuffix(Path path) {
        String fullPath = path.toAbsolutePath().toString();
        fullPath = fullPath.substring(fullPath.indexOf('/', fullPath.indexOf('/', fullPath.indexOf('/')+1)+1)+1, fullPath.lastIndexOf('.'));
        return fullPath;
    }

    public static Map<String, Map<String, String>> parseIniFile(File fileToParse) throws IOException {
        Ini ini = new Ini(fileToParse);
        return ini.entrySet().stream()
            .collect(Collectors.toMap(Map.Entry::getKey, Map.Entry::getValue));
    }
}

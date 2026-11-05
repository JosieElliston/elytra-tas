// Compile .pyj files with the Pyjinn bundled in the Minescript mod, without the game.
//
// usage: minescript/pyjinn_check.sh FILE.pyj...
//
// Runs the same parse + compile Minescript does at `\script` time
// (PyjinnScript.loadScript), so unsupported syntax fails here instead of in game.
// It does not run the script: imports, Java class names and runtime errors are
// not checked.

import com.google.gson.JsonElement;
import java.nio.file.Files;
import java.nio.file.Path;
import org.pyjinn.interpreter.Script;
import org.pyjinn.parser.PyjinnParser;

public class PyjinnCheck {
  public static void main(String[] args) throws Exception {
    int failed = 0;
    for (String file : args) {
      String name = Path.of(file).getFileName().toString();
      try {
        JsonElement ast = PyjinnParser.parse(name, Files.readString(Path.of(file)));
        new Script().compile(ast, name);
        System.out.println("ok: " + file);
      } catch (Exception e) {
        failed++;
        Throwable root = e;
        while (root.getCause() != null) root = root.getCause();
        System.out.println("FAIL: " + file + ": " + e.getMessage().lines().findFirst().orElse(""));
        if (root != e) System.out.println("  cause: " + root);
      }
    }
    System.exit(failed == 0 ? 0 : 1);
  }
}
